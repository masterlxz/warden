//! The opencode as a `CodeEngine`: talks HTTP to an `opencode serve` the `Launcher` hands out for the project's folder.
//!
//! One task is: make (or reuse) a session → open the event stream and wait until it is live → `POST prompt_async` →
//! follow the events (`Tracker`) until the session goes idle, answering each permission ask with the person's yes or no.
//! The stream is opened *before* the prompt is sent, so nothing the engine does is missed.
//!
//! Every route takes `?directory=`, which picks the project the engine works in; the server is also started in that
//! folder, so a request that forgot it would still land in the right place.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::tracker::{PermissionAsk, Signal, Tracker};
use super::{CodeEngine, CodeEvent, CodeMode, TurnOutcome, TurnRequest};
use crate::tool::{Answer, ApprovalRequest, Approver};

/// How long one permission ask waits for the person; no answer counts as no (like the project shell and the SSH hosts).
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
/// The user name the server's basic auth expects, whatever the password.
const AUTH_USER: &str = "opencode";
/// What the engine reports when a task is stopped on purpose: the work done so far is kept, so it isn't a failure.
const ABORTED: &str = "MessageAbortedError";

/// Where a server for a folder is listening.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub password: Option<String>,
    /// Held for as long as a task uses the server: the launcher doesn't end a server someone holds a lease on.
    pub lease: Option<Arc<()>>,
}

/// Hands out the server of a folder — starting it if it isn't up. The real one runs `opencode serve`; tests point at a
/// fake.
#[async_trait]
pub trait Launcher: Send + Sync {
    async fn endpoint(&self, workdir: &str) -> anyhow::Result<Endpoint>;
}

/// A session the engine no longer knows (its data was removed): a new one is opened instead.
#[derive(Debug)]
struct SessionGone;
impl std::fmt::Display for SessionGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the engine no longer has this session")
    }
}
impl std::error::Error for SessionGone {}

/// What a new session may do without asking: look, never change. Everything else asks (the last matching rule wins).
fn session_permissions() -> Value {
    let mut rules = vec![json!({"permission": "*", "pattern": "*", "action": "ask"})];
    rules.extend(["read", "glob", "grep", "list"].map(|tool| json!({"permission": tool, "pattern": "*", "action": "allow"})));
    Value::Array(rules)
}

type Events = Pin<Box<dyn Stream<Item = anyhow::Result<Value>> + Send>>;

pub(super) struct Client {
    http: reqwest::Client,
    endpoint: Endpoint,
    directory: String,
}

impl Client {
    pub(super) fn new(endpoint: Endpoint, directory: &str) -> Self {
        // No overall timeout: the event stream lives as long as the task does.
        Self { http: reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().unwrap_or_default(), endpoint, directory: directory.to_string() }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.endpoint.base_url.trim_end_matches('/'));
        let builder = self.http.request(method, url).query(&[("directory", self.directory.as_str())]);
        match &self.endpoint.password {
            Some(password) => builder.basic_auth(AUTH_USER, Some(password)),
            None => builder,
        }
    }

    pub(super) async fn create_session(&self, title: &str) -> anyhow::Result<String> {
        let response = self.request(reqwest::Method::POST, "/session").json(&json!({"title": title, "permission": session_permissions()})).send().await.context("reaching the opencode")?;
        let status = response.status();
        if !status.is_success() {
            bail!("the opencode refused to open a session ({status})");
        }
        let body: Value = response.json().await.context("reading the opencode's session")?;
        body["id"].as_str().map(str::to_string).ok_or_else(|| anyhow!("the opencode's session had no id"))
    }

    /// The event stream, live: returned only once the server has said it is connected.
    pub(super) async fn events(&self) -> anyhow::Result<Events> {
        let response = self.request(reqwest::Method::GET, "/event").send().await.context("reaching the opencode")?;
        let status = response.status();
        if !status.is_success() {
            bail!("the opencode refused the event stream ({status})");
        }
        let mut stream = response.bytes_stream().eventsource().map(|event| match event {
            Ok(event) => serde_json::from_str::<Value>(&event.data).map_err(|e| anyhow!("an unreadable event from the opencode: {e}")),
            Err(e) => Err(anyhow!("the opencode's event stream broke: {e}")),
        });
        loop {
            match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
                Err(_) => bail!("the opencode's event stream never said it was connected"),
                Ok(None) => bail!("the opencode closed the event stream at once"),
                Ok(Some(event)) => {
                    if event?["type"].as_str() == Some("server.connected") {
                        return Ok(Box::pin(stream));
                    }
                }
            }
        }
    }

    async fn prompt(&self, session: &str, text: &str, system: Option<&str>) -> anyhow::Result<()> {
        let mut body = json!({"parts": [{"type": "text", "text": text}]});
        if let Some(system) = system.filter(|s| !s.trim().is_empty()) {
            body["system"] = json!(system);
        }
        let response = self.request(reqwest::Method::POST, &format!("/session/{session}/prompt_async")).json(&body).send().await.context("reaching the opencode")?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SessionGone.into());
        }
        if !status.is_success() {
            bail!("the opencode refused the task ({status})");
        }
        Ok(())
    }

    async fn answer(&self, permission: &str, allowed: bool) -> anyhow::Result<()> {
        let reply = if allowed { "once" } else { "reject" };
        let response = self.request(reqwest::Method::POST, &format!("/permission/{permission}/reply")).json(&json!({"reply": reply})).send().await.context("reaching the opencode")?;
        if !response.status().is_success() {
            bail!("the opencode refused the answer to a permission ask ({})", response.status());
        }
        Ok(())
    }

    async fn abort(&self, session: &str) -> anyhow::Result<()> {
        let response = self.request(reqwest::Method::POST, &format!("/session/{session}/abort")).send().await.context("reaching the opencode")?;
        if !response.status().is_success() {
            bail!("the opencode refused to stop ({})", response.status());
        }
        Ok(())
    }
}

pub struct OpencodeEngine {
    launcher: Arc<dyn Launcher>,
    /// The "always" answers, by session (a session is a conversation): `(permission, pattern)` pairs that are a yes
    /// without asking. Only in memory — a restart forgets them — and ours rather than the engine's own, so they can be
    /// seen and dropped from here.
    always: Mutex<HashMap<String, Vec<(String, String)>>>,
}

impl OpencodeEngine {
    pub fn new(launcher: Arc<dyn Launcher>) -> Self {
        Self { launcher, always: Mutex::default() }
    }

    /// Whether the person already said "always" to what this ask would do: the same kind of action, and every thing it
    /// touches matched by something they allowed.
    fn is_allowed(&self, session: &str, ask: &PermissionAsk) -> bool {
        let always = self.always.lock().unwrap_or_else(|e| e.into_inner());
        let Some(allowed) = always.get(session) else { return false };
        !ask.patterns.is_empty() && ask.patterns.iter().all(|touched| allowed.iter().any(|(permission, pattern)| *permission == ask.permission && matches_pattern(pattern, touched)))
    }

    /// Whether the engine may do what it asks to. The mode decides first (and is looked at again whenever the person
    /// changes it, so an ask waiting at the modal when they switch to "accept all" or to "plan" is answered by the new
    /// mode and the modal goes away); then what they said "always" to; then the person.
    async fn decide(&self, session: &str, ask: &PermissionAsk, target: &str, approver: Option<&Arc<dyn Approver>>, mode: &mut watch::Receiver<CodeMode>) -> bool {
        let mut watching = true;
        loop {
            let current = *mode.borrow_and_update();
            if let Some(decision) = current.decides(&ask.permission) {
                return decision;
            }
            if self.is_allowed(session, ask) {
                return true;
            }
            let Some(approver) = approver else { return false };
            let request = ApprovalRequest::new(target.to_string(), ask.permission.clone(), ask.patterns.join(", "));
            let covers = Some(ask.always.join(", ")).filter(|c| !c.is_empty());
            tokio::select! {
                answer = tokio::time::timeout(APPROVAL_TIMEOUT, approver.ask(request, covers.as_deref())) => {
                    let answer = answer.unwrap_or(Answer::Reject);
                    if answer == Answer::Always {
                        self.remember(session, ask);
                    }
                    return answer != Answer::Reject;
                }
                // Dropping the question withdraws it from the person's screen.
                changed = mode.changed(), if watching => {
                    // Nobody can change the mode any more: stop listening for it rather than spin.
                    watching = changed.is_ok();
                }
            }
        }
    }

    fn remember(&self, session: &str, ask: &PermissionAsk) {
        let mut always = self.always.lock().unwrap_or_else(|e| e.into_inner());
        let allowed = always.entry(session.to_string()).or_default();
        for pattern in &ask.always {
            let entry = (ask.permission.clone(), pattern.clone());
            if !allowed.contains(&entry) {
                allowed.push(entry);
            }
        }
    }
}

/// The engine's own pattern language: `*` is any run of characters, and a final ` *` also matches the bare command
/// (`git status *` allows `git status` and `git status -s`, not `git statusx`).
fn matches_pattern(pattern: &str, text: &str) -> bool {
    fn glob(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|skip| glob(rest, &text[skip..])),
            Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
        }
    }
    let text: Vec<char> = text.chars().collect();
    let full: Vec<char> = pattern.chars().collect();
    if glob(&full, &text) {
        return true;
    }
    match pattern.strip_suffix(" *") {
        Some(bare) => glob(&bare.chars().collect::<Vec<_>>(), &text),
        None => false,
    }
}

#[async_trait]
impl CodeEngine for OpencodeEngine {
    async fn run_turn(&self, request: TurnRequest, approver: Option<Arc<dyn Approver>>, on_event: &mut (dyn FnMut(CodeEvent) + Send)) -> anyhow::Result<TurnOutcome> {
        let client = Client::new(self.launcher.endpoint(&request.workdir).await?, &request.workdir);
        let mut session = match &request.session_id {
            Some(id) => id.clone(),
            None => client.create_session(&request.title).await?,
        };
        let mut events = client.events().await?;
        let mut mode = request.mode.clone();
        // The mode's own instructions (the plan mode's) go with the task, besides the project's.
        let mode_note = mode.borrow().instructions();
        let system_text = match (request.system.as_deref(), mode_note) {
            (Some(system), Some(note)) => Some(format!("{system}\n\n{note}")),
            (None, Some(note)) => Some(note.to_string()),
            (system, None) => system.map(str::to_string),
        };
        let system = system_text.as_deref();
        if let Err(err) = client.prompt(&session, &request.prompt, system).await {
            if request.session_id.is_none() || !err.is::<SessionGone>() {
                return Err(err);
            }
            session = client.create_session(&request.title).await?;
            client.prompt(&session, &request.prompt, system).await?;
        }

        on_event(CodeEvent::Session(session.clone()));
        let mut tracker = Tracker::new(&session);
        loop {
            let Some(event) = events.next().await else { bail!("the connection to the opencode was lost before the task ended") };
            for signal in tracker.feed(&event?) {
                match signal {
                    Signal::Event(event) => on_event(event),
                    Signal::Ask(ask) => {
                        let allowed = self.decide(&session, &ask, &request.target, approver.as_ref(), &mut mode).await;
                        client.answer(&ask.id, allowed).await?;
                    }
                    Signal::Idle => return Ok(TurnOutcome { session_id: session, text: tracker.text(), tools_used: tracker.tools_used().to_vec() }),
                    Signal::Failed(reason) if reason == ABORTED => return Ok(TurnOutcome { session_id: session, text: tracker.text(), tools_used: tracker.tools_used().to_vec() }),
                    Signal::Failed(reason) => bail!("the opencode failed: {reason}"),
                }
            }
        }
    }

    async fn abort(&self, workdir: &str, session_id: &str) -> anyhow::Result<()> {
        Client::new(self.launcher.endpoint(workdir).await?, workdir).abort(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    use super::*;

    const S: &str = "ses_root";

    /// One thing the fake's event stream does after the task is sent.
    enum Step {
        Event(Value),
        /// Holds the stream until a permission ask has been answered.
        WaitForAnswer,
    }

    struct Fake {
        addr: String,
        /// `(method, path with query, body)` of every request the engine made.
        requests: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl Fake {
        fn calls(&self, method: &str, path: &str) -> Vec<String> {
            self.requests.lock().unwrap().iter().filter(|(m, p, _)| m == method && p.split('?').next() == Some(path)).map(|(_, _, body)| body.clone()).collect()
        }
        fn launcher(&self, password: Option<&str>) -> Arc<dyn Launcher> {
            struct Fixed(Endpoint);
            #[async_trait]
            impl Launcher for Fixed {
                async fn endpoint(&self, _: &str) -> anyhow::Result<Endpoint> {
                    Ok(self.0.clone())
                }
            }
            Arc::new(Fixed(Endpoint { base_url: format!("http://{}", self.addr), password: password.map(str::to_string), lease: None }))
        }
    }

    /// A tiny opencode: `POST /session` answers `ses_root` (or 404s a prompt for a session in `gone`), `GET /event`
    /// says it is connected and then plays `steps` once a task has been sent, the rest just record.
    async fn serve(steps: Vec<Step>, gone: Vec<&str>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let requests: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
        let (steps, gone) = (Arc::new(Mutex::new(Some(steps))), Arc::new(gone.into_iter().map(str::to_string).collect::<Vec<_>>()));
        let (prompted, answered) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let (seen, steps, gone, prompted, answered) = (seen.clone(), steps.clone(), gone.clone(), prompted.clone(), answered.clone());
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buffer = [0u8; 4096];
                    let (head, body) = loop {
                        let Ok(read) = socket.read(&mut buffer).await else { return };
                        if read == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buffer[..read]);
                        let text = String::from_utf8_lossy(&raw).to_string();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let wanted = head.to_lowercase().split("content-length:").nth(1).and_then(|v| v.lines().next()?.trim().parse::<usize>().ok()).unwrap_or(0);
                            if body.len() >= wanted {
                                break (head.to_string(), body.to_string());
                            }
                        }
                    };
                    let mut first = head.lines().next().unwrap_or_default().split(' ');
                    let (method, path) = (first.next().unwrap_or_default().to_string(), first.next().unwrap_or_default().to_string());
                    seen.lock().unwrap().push((method.clone(), path.clone(), body));
                    // The head too, under its own marker, for the tests that check the password was sent.
                    seen.lock().unwrap().push(("HEAD".into(), path.clone(), head.to_lowercase()));
                    let route = path.split('?').next().unwrap_or_default().to_string();
                    let json = |status: &str, body: &str| format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let reply = match (method.as_str(), route.as_str()) {
                        ("POST", "/session") => json("200 OK", &format!(r#"{{"id":"{S}"}}"#)),
                        ("POST", p) if p.ends_with("/prompt_async") => {
                            if gone.iter().any(|g| p.contains(g.as_str())) {
                                json("404 Not Found", "{}")
                            } else {
                                prompted.notify_one();
                                "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".to_string()
                            }
                        }
                        ("POST", p) if p.starts_with("/permission/") => {
                            answered.notify_one();
                            json("200 OK", "true")
                        }
                        ("POST", p) if p.ends_with("/abort") => json("200 OK", "true"),
                        ("GET", "/event") => {
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n").await;
                            let send = |event: Value| format!("data: {event}\n\n");
                            let _ = socket.write_all(send(json!({"type": "server.connected", "properties": {}})).as_bytes()).await;
                            let Some(steps) = steps.lock().unwrap().take() else { return };
                            prompted.notified().await;
                            for step in steps {
                                match step {
                                    Step::Event(event) => {
                                        let _ = socket.write_all(send(event).as_bytes()).await;
                                    }
                                    Step::WaitForAnswer => answered.notified().await,
                                }
                            }
                            // Held open like the real one: the engine ends at `session.idle`, not at EOF.
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            return;
                        }
                        _ => json("404 Not Found", "{}"),
                    };
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Fake { addr, requests }
    }

    fn event(kind: &str, properties: Value) -> Step {
        Step::Event(json!({"type": kind, "properties": properties}))
    }
    fn assistant() -> Step {
        event("message.updated", json!({"info": {"id": "msg_a", "role": "assistant", "sessionID": S}}))
    }
    fn answer(text: &str) -> Step {
        event("message.part.updated", json!({"sessionID": S, "part": {"id": "prt_1", "messageID": "msg_a", "sessionID": S, "type": "text", "text": text}}))
    }
    fn idle() -> Step {
        event("session.idle", json!({"sessionID": S}))
    }
    fn ask(id: &str, command: &str) -> Step {
        event("permission.asked", json!({"id": id, "sessionID": S, "permission": "bash", "patterns": [command], "metadata": {}, "always": []}))
    }

    fn request(session: Option<&str>) -> TurnRequest {
        request_in(session, CodeMode::Manual)
    }

    /// A task in `mode` that nobody changes.
    fn request_in(session: Option<&str>, mode: CodeMode) -> TurnRequest {
        TurnRequest { workdir: "/home/me/repo".into(), session_id: session.map(str::to_string), prompt: "fix the bug".into(), title: "Repo".into(), target: "Repo".into(), system: Some("Be brief.".into()), mode: watch::channel(mode).1 }
    }

    struct Says {
        yes: bool,
        asked: Mutex<Vec<ApprovalRequest>>,
    }
    #[async_trait]
    impl Approver for Says {
        async fn approve(&self, request: ApprovalRequest) -> bool {
            self.asked.lock().unwrap().push(request);
            self.yes
        }
    }
    fn approver(yes: bool) -> Arc<Says> {
        Arc::new(Says { yes, asked: Mutex::default() })
    }

    async fn run(engine: &OpencodeEngine, request: TurnRequest, approver: Option<Arc<dyn Approver>>) -> (anyhow::Result<TurnOutcome>, Vec<CodeEvent>) {
        let mut shown = Vec::new();
        let outcome = tokio::time::timeout(Duration::from_secs(10), engine.run_turn(request, approver, &mut |e| shown.push(e))).await.expect("the task ended");
        (outcome, shown)
    }

    #[tokio::test]
    async fn a_task_opens_a_session_in_the_folder_sends_the_prompt_and_ends_at_idle_with_the_answer() {
        let fake = serve(vec![assistant(), answer("Fixed it."), idle()], vec![]).await;
        let engine = OpencodeEngine::new(fake.launcher(Some("pw")));
        let (outcome, shown) = run(&engine, request(None), None).await;
        let outcome = outcome.unwrap();
        assert_eq!((outcome.session_id.as_str(), outcome.text.as_str()), (S, "Fixed it."));
        assert_eq!(shown, [CodeEvent::Session(S.into()), CodeEvent::Text("Fixed it.".into())], "the session is told as soon as there is one, so the task can be stopped");

        let created: Value = serde_json::from_str(&fake.calls("POST", "/session")[0]).unwrap();
        assert_eq!(created["title"], "Repo");
        let rules = created["permission"].as_array().unwrap();
        assert_eq!(rules[0], json!({"permission": "*", "pattern": "*", "action": "ask"}), "everything asks first…");
        assert!(rules.iter().any(|r| r["permission"] == "read" && r["action"] == "allow"), "…except looking");
        assert!(!rules.iter().any(|r| r["permission"] == "bash" && r["action"] == "allow"));
        let prompt: Value = serde_json::from_str(&fake.calls("POST", "/session/ses_root/prompt_async")[0]).unwrap();
        assert_eq!(prompt["parts"][0]["text"], "fix the bug");
        assert_eq!(prompt["system"], "Be brief.", "the project's instructions go with the task");

        let log = fake.requests.lock().unwrap().clone();
        assert!(log.iter().filter(|(m, ..)| m != "HEAD").all(|(_, p, _)| p.contains("directory=%2Fhome%2Fme%2Frepo")), "every request names the folder: {log:?}");
        assert!(log.iter().filter(|(m, ..)| m == "HEAD").all(|(_, _, head)| head.contains("authorization: basic")), "and carries the password");
    }

    #[tokio::test]
    async fn a_session_the_conversation_already_has_is_reused_not_opened_again() {
        let fake = serve(vec![assistant(), answer("ok"), idle()], vec![]).await;
        let engine = OpencodeEngine::new(fake.launcher(None));
        let (outcome, _) = run(&engine, request(Some(S)), None).await;
        assert_eq!(outcome.unwrap().session_id, S);
        assert!(fake.calls("POST", "/session").is_empty());
        assert_eq!(fake.calls("POST", "/session/ses_root/prompt_async").len(), 1);
    }

    #[tokio::test]
    async fn a_session_the_engine_lost_is_replaced_and_the_task_still_runs() {
        let fake = serve(vec![assistant(), answer("ok"), idle()], vec!["ses_old"]).await;
        let engine = OpencodeEngine::new(fake.launcher(None));
        let (outcome, _) = run(&engine, request(Some("ses_old")), None).await;
        assert_eq!(outcome.unwrap().session_id, S, "the new session is what the conversation keeps");
        assert_eq!(fake.calls("POST", "/session").len(), 1);
    }

    #[tokio::test]
    async fn every_permission_ask_goes_to_the_person_and_their_answer_goes_back() {
        for yes in [true, false] {
            let fake = serve(vec![assistant(), ask("per_1", "cargo test"), Step::WaitForAnswer, answer("done"), idle()], vec![]).await;
            let engine = OpencodeEngine::new(fake.launcher(None));
            let person = approver(yes);
            let (outcome, _) = run(&engine, request(None), Some(person.clone())).await;
            outcome.unwrap();
            assert_eq!(person.asked.lock().unwrap().as_slice(), [ApprovalRequest::new("Repo", "bash", "cargo test")]);
            let reply: Value = serde_json::from_str(&fake.calls("POST", "/permission/per_1/reply")[0]).unwrap();
            assert_eq!(reply["reply"], if yes { "once" } else { "reject" });
        }
    }

    fn ask_always(id: &str, permission: &str, command: &str, always: &str) -> Step {
        event("permission.asked", json!({"id": id, "sessionID": S, "permission": permission, "patterns": [command], "metadata": {}, "always": [always]}))
    }

    /// Answers "always" to the first ask and records what it was asked and what it was offered.
    struct Forever {
        asked: Mutex<Vec<(ApprovalRequest, Option<String>)>>,
        answer: Answer,
    }
    #[async_trait]
    impl Approver for Forever {
        async fn approve(&self, _request: ApprovalRequest) -> bool {
            unreachable!("the code engine asks with `ask`")
        }
        async fn ask(&self, request: ApprovalRequest, always: Option<&str>) -> Answer {
            self.asked.lock().unwrap().push((request, always.map(str::to_string)));
            self.answer
        }
    }

    #[tokio::test]
    async fn an_always_answer_is_remembered_for_the_same_kind_of_ask_and_nothing_else() {
        let steps = vec![
            assistant(),
            ask_always("per_1", "bash", "git status -s", "git status *"),
            Step::WaitForAnswer,
            ask_always("per_2", "bash", "git status", "git status *"),
            Step::WaitForAnswer,
            ask_always("per_3", "bash", "git push", "git push *"),
            Step::WaitForAnswer,
            ask_always("per_4", "edit", "git status -s", "*"),
            Step::WaitForAnswer,
            idle(),
        ];
        let fake = serve(steps, vec![]).await;
        let engine = OpencodeEngine::new(fake.launcher(None));
        let person = Arc::new(Forever { asked: Mutex::default(), answer: Answer::Always });
        run(&engine, request(None), Some(person.clone())).await.0.unwrap();

        let asked = person.asked.lock().unwrap();
        let details: Vec<&str> = asked.iter().map(|(r, _)| r.detail.as_str()).collect();
        assert_eq!(details, ["git status -s", "git push", "git status -s"], "`git status` was covered, the push and the other kind of action were not");
        assert_eq!(asked[0].1.as_deref(), Some("git status *"), "the person is told what \"always\" would cover");
        for id in ["per_1", "per_2", "per_3", "per_4"] {
            let reply: Value = serde_json::from_str(&fake.calls("POST", &format!("/permission/{id}/reply"))[0]).unwrap();
            assert_eq!(reply["reply"], "once", "{id}: the engine is always told yes once; the remembering is ours");
        }
    }

    #[tokio::test]
    async fn a_plain_yes_or_no_is_not_remembered() {
        for answer in [Answer::Once, Answer::Reject] {
            let steps = vec![assistant(), ask_always("per_1", "bash", "ls", "ls *"), Step::WaitForAnswer, ask_always("per_2", "bash", "ls", "ls *"), Step::WaitForAnswer, idle()];
            let fake = serve(steps, vec![]).await;
            let engine = OpencodeEngine::new(fake.launcher(None));
            let person = Arc::new(Forever { asked: Mutex::default(), answer });
            run(&engine, request(None), Some(person.clone())).await.0.unwrap();
            assert_eq!(person.asked.lock().unwrap().len(), 2, "{answer:?} asks again");
        }
    }

    #[tokio::test]
    async fn an_always_without_a_suggestion_from_the_engine_is_not_offered_or_remembered() {
        let steps = vec![assistant(), ask("per_1", "ls"), Step::WaitForAnswer, ask("per_2", "ls"), Step::WaitForAnswer, idle()];
        let fake = serve(steps, vec![]).await;
        let engine = OpencodeEngine::new(fake.launcher(None));
        let person = Arc::new(Forever { asked: Mutex::default(), answer: Answer::Always });
        run(&engine, request(None), Some(person.clone())).await.0.unwrap();
        let asked = person.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].1, None, "nothing to offer");
    }

    /// What the engine was told for one ask, by id.
    fn reply(fake: &Fake, id: &str) -> String {
        let reply: Value = serde_json::from_str(&fake.calls("POST", &format!("/permission/{id}/reply"))[0]).unwrap();
        reply["reply"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn the_mode_decides_what_asks_and_what_does_not() {
        // (mode, answer to an edit, answer to a command, how many times the person was asked)
        let cases = [
            (CodeMode::Manual, "once", "once", 2),
            (CodeMode::AcceptEdits, "once", "once", 1),
            (CodeMode::AcceptAll, "once", "once", 0),
            (CodeMode::Plan, "reject", "reject", 0),
        ];
        for (mode, edit, command, asked) in cases {
            let steps = vec![assistant(), ask_always("per_1", "edit", "src/main.rs", "*"), Step::WaitForAnswer, ask("per_2", "cargo test"), Step::WaitForAnswer, idle()];
            let fake = serve(steps, vec![]).await;
            let engine = OpencodeEngine::new(fake.launcher(None));
            let person = approver(true);
            run(&engine, request_in(None, mode), Some(person.clone())).await.0.unwrap();
            assert_eq!((reply(&fake, "per_1").as_str(), reply(&fake, "per_2").as_str()), (edit, command), "{mode:?}");
            assert_eq!(person.asked.lock().unwrap().len(), asked, "{mode:?}");
        }
    }

    #[tokio::test]
    async fn plan_mode_tells_the_engine_to_answer_with_a_plan_and_the_other_modes_do_not() {
        for (mode, said) in [(CodeMode::Plan, true), (CodeMode::Manual, false), (CodeMode::AcceptAll, false)] {
            let fake = serve(vec![assistant(), answer("ok"), idle()], vec![]).await;
            run(&OpencodeEngine::new(fake.launcher(None)), request_in(None, mode), None).await.0.unwrap();
            let prompt: Value = serde_json::from_str(&fake.calls("POST", "/session/ses_root/prompt_async")[0]).unwrap();
            let system = prompt["system"].as_str().unwrap();
            assert!(system.starts_with("Be brief."), "the project's own instructions stay");
            assert_eq!(system.contains("Plan mode"), said, "{mode:?}");
        }
    }

    /// Never answers, and says it was asked.
    struct Hangs(tokio::sync::Notify);
    #[async_trait]
    impl Approver for Hangs {
        async fn approve(&self, _request: ApprovalRequest) -> bool {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn changing_the_mode_while_an_ask_waits_answers_it_with_the_new_mode() {
        for (to, expected) in [(CodeMode::AcceptAll, "once"), (CodeMode::Plan, "reject")] {
            let fake = serve(vec![assistant(), ask("per_1", "cargo test"), Step::WaitForAnswer, idle()], vec![]).await;
            let engine = OpencodeEngine::new(fake.launcher(None));
            let modes = super::super::CodeModes::default();
            let mut task = request(None);
            task.mode = modes.subscribe("c1");
            let person = Arc::new(Hangs(tokio::sync::Notify::new()));
            let waiting = person.clone();
            tokio::spawn(async move {
                waiting.0.notified().await;
                modes.set("c1", to);
            });
            run(&engine, task, Some(person)).await.0.unwrap();
            assert_eq!(reply(&fake, "per_1"), expected, "switched to {to:?} with the ask open");
        }
    }

    #[test]
    fn the_engines_patterns_match_the_way_it_writes_them() {
        assert!(matches_pattern("git status *", "git status -s"));
        assert!(matches_pattern("git status *", "git status"), "a final ` *` also allows the bare command");
        assert!(!matches_pattern("git status *", "git statusx"));
        assert!(!matches_pattern("git status *", "rm -rf x"));
        assert!(matches_pattern("*", "anything at all"));
        assert!(matches_pattern("src/*.rs", "src/main.rs"));
        assert!(!matches_pattern("src/*.rs", "src/main.py"));
        assert!(matches_pattern("ls", "ls") && !matches_pattern("ls", "ls -la"), "no star, exact");
    }

    #[tokio::test]
    async fn with_nobody_to_ask_the_answer_is_no() {
        let fake = serve(vec![ask("per_1", "rm -rf /"), Step::WaitForAnswer, idle()], vec![]).await;
        let engine = OpencodeEngine::new(fake.launcher(None));
        run(&engine, request(None), None).await.0.unwrap();
        let reply: Value = serde_json::from_str(&fake.calls("POST", "/permission/per_1/reply")[0]).unwrap();
        assert_eq!(reply["reply"], "reject");
    }

    #[tokio::test]
    async fn an_engine_error_fails_the_task_but_a_stop_on_purpose_keeps_the_work() {
        let failing = serve(vec![event("session.error", json!({"sessionID": S, "error": {"name": "APIError", "data": {"message": "no credits"}}}))], vec![]).await;
        let (outcome, _) = run(&OpencodeEngine::new(failing.launcher(None)), request(None), None).await;
        assert!(outcome.err().unwrap().to_string().contains("no credits"));

        let stopped = serve(vec![assistant(), answer("half"), event("session.error", json!({"sessionID": S, "error": {"name": "MessageAbortedError"}}))], vec![]).await;
        let (outcome, _) = run(&OpencodeEngine::new(stopped.launcher(None)), request(None), None).await;
        assert_eq!(outcome.unwrap().text, "half");
    }

    #[tokio::test]
    async fn stopping_a_task_aborts_its_session_and_nothing_listening_is_an_error_not_a_hang() {
        let fake = serve(vec![], vec![]).await;
        OpencodeEngine::new(fake.launcher(None)).abort("/home/me/repo", S).await.unwrap();
        assert_eq!(fake.calls("POST", "/session/ses_root/abort").len(), 1);

        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap().to_string();
        drop(probe);
        let dead = Fake { addr, requests: Arc::default() };
        let (outcome, _) = run(&OpencodeEngine::new(dead.launcher(None)), request(None), None).await;
        assert!(outcome.is_err());
    }
}
