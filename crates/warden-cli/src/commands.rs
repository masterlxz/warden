//! Slash-command parsing for the interactive REPL (`interactive.rs`) — pure text-in, typed-value-
//! out, with no `ratatui`/terminal dependency, so it's unit-testable the same way `LineEditor` is.

use warden_bootstrap::Provider;

/// One recognized slash-command, already split into its typed arguments.
pub enum Command {
    Exit,
    Help,
    Usage,
    ModelsList,
    ModelsUse(String),
    ModelsReset,
    ModelsAdd,
    ModelsEdit(String),
    ModelsRemove(String),
    AgentsList,
    /// `None` is `/agents use none` — clears the session's active agent.
    AgentsUse(Option<String>),
    AgentsCreate,
    AgentsEdit(String),
    AgentsRemove(String),
    SyncStatus,
    SyncPush,
    SyncPull,
    SyncPairShow,
    SyncPairJoin(String),
    SyncGitPush,
    SyncGitPull,
}

/// What a line of input turned out to be, once checked against the slash-command grammar.
pub enum ParseOutcome {
    /// Doesn't start with `/` at all — treat as a normal chat message.
    NotACommand,
    Recognized(Command),
    /// Starts with `/` but doesn't match any known command/argument shape — must never be
    /// forwarded to the model as a chat message; the caller renders an error instead.
    Unrecognized(String),
}

pub fn parse_command(input: &str) -> ParseOutcome {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return ParseOutcome::NotACommand;
    }

    let mut parts = trimmed[1..].split_whitespace();
    let Some(head) = parts.next() else {
        return ParseOutcome::Unrecognized(trimmed.to_string());
    };
    let rest: Vec<&str> = parts.collect();

    let command = match (head.to_ascii_lowercase().as_str(), rest.as_slice()) {
        ("exit", []) | ("quit", []) => Command::Exit,
        ("help", []) => Command::Help,
        ("usage", []) => Command::Usage,
        ("models", []) => Command::ModelsList,
        ("models", ["use", id]) => Command::ModelsUse(id.to_string()),
        ("models", ["reset"]) => Command::ModelsReset,
        ("models", ["add"]) => Command::ModelsAdd,
        ("models", ["edit", id]) => Command::ModelsEdit(id.to_string()),
        ("models", ["remove", id]) => Command::ModelsRemove(id.to_string()),
        ("agents", []) => Command::AgentsList,
        ("agents", ["use", "none"]) => Command::AgentsUse(None),
        ("agents", ["use", id]) => Command::AgentsUse(Some(id.to_string())),
        ("agents", ["create"]) => Command::AgentsCreate,
        ("agents", ["edit", id]) => Command::AgentsEdit(id.to_string()),
        ("agents", ["remove", id]) => Command::AgentsRemove(id.to_string()),
        ("sync", []) => Command::SyncStatus,
        ("sync", ["push"]) => Command::SyncPush,
        ("sync", ["pull"]) => Command::SyncPull,
        ("sync", ["pair"]) => Command::SyncPairShow,
        ("sync", ["pair", code]) => Command::SyncPairJoin(code.to_string()),
        ("sync", ["git", "push"]) => Command::SyncGitPush,
        ("sync", ["git", "pull"]) => Command::SyncGitPull,
        _ => return ParseOutcome::Unrecognized(trimmed.to_string()),
    };
    ParseOutcome::Recognized(command)
}

/// Parses a provider `kind` the way a user would type it (matching the config file's own
/// `#[serde(rename_all = "snake_case")]` spelling) — there's no `FromStr` on `Provider` itself
/// since its only other consumer (`toml`) goes through `serde`, not `str::parse`.
pub fn parse_provider_kind(input: &str) -> Option<Provider> {
    match input.trim().to_ascii_lowercase().as_str() {
        "gemini" => Some(Provider::Gemini),
        "openai" => Some(Provider::Openai),
        "anthropic" => Some(Provider::Anthropic),
        "openai_compatible" | "openai-compatible" => Some(Provider::OpenaiCompatible),
        _ => None,
    }
}

/// The reverse of `parse_provider_kind` — used to display a provider's kind back to the user.
pub fn kind_label(kind: Provider) -> &'static str {
    match kind {
        Provider::Gemini => "gemini",
        Provider::Openai => "openai",
        Provider::Anthropic => "anthropic",
        Provider::OpenaiCompatible => "openai_compatible",
    }
}

const TOP_LEVEL_COMMANDS: &[&str] = &["exit", "quit", "help", "usage", "models", "agents", "sync"];
const MODELS_SUBCOMMANDS: &[&str] = &["use", "reset", "add", "edit", "remove"];
const AGENTS_SUBCOMMANDS: &[&str] = &["use", "create", "edit", "remove"];
const SYNC_SUBCOMMANDS: &[&str] = &["push", "pull", "pair", "git"];

/// Parses the word currently being typed (the last whitespace-separated token) out of a
/// `/`-prefixed `input`, along with its candidate completions from the fixed part of the grammar
/// (`parse_command`'s own vocabulary — command names one level, subcommand names the next).
/// Returns `None` once a subcommand that takes a free-form argument (an id, a persona, ...) has
/// already been typed — there's no fixed list to suggest there, and this grammar never branches
/// again past the second word. `ghost_suggestion` (its only caller) needs the exact typed
/// fragment as well as the candidates, to compute the completion suffix.
fn current_word(input: &str) -> Option<(&str, Vec<&'static str>)> {
    let after_slash = input.strip_prefix('/')?;
    if let Some(space_idx) = after_slash.find(' ') {
        let head = after_slash[..space_idx].to_ascii_lowercase();
        let rest = &after_slash[space_idx + 1..];
        if rest.contains(' ') {
            return None;
        }
        let subcommands: &[&str] = match head.as_str() {
            "models" => MODELS_SUBCOMMANDS,
            "agents" => AGENTS_SUBCOMMANDS,
            "sync" => SYNC_SUBCOMMANDS,
            _ => return None,
        };
        let typed_lower = rest.to_ascii_lowercase();
        Some((rest, subcommands.iter().copied().filter(|c| c.starts_with(typed_lower.as_str())).collect()))
    } else {
        let typed_lower = after_slash.to_ascii_lowercase();
        Some((after_slash, TOP_LEVEL_COMMANDS.iter().copied().filter(|c| c.starts_with(typed_lower.as_str())).collect()))
    }
}

/// The live "ghost" completion to show/accept for `input` (the whole line typed so far) — the
/// remaining characters that would complete its last word to a single unambiguous candidate.
/// `None` when there's no `/`-command context, the word is already fully typed, or more than one
/// candidate remains (ambiguous — nothing single to preview or accept on Tab).
pub fn ghost_suggestion(input: &str) -> Option<String> {
    let (typed, candidates) = current_word(input)?;
    if candidates.len() != 1 {
        return None;
    }
    let only = candidates[0];
    if only.len() <= typed.len() {
        return None;
    }
    Some(only[typed.len()..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_recognized(input: &str) -> Command {
        match parse_command(input) {
            ParseOutcome::Recognized(command) => command,
            ParseOutcome::NotACommand => panic!("expected a command, got NotACommand for {input:?}"),
            ParseOutcome::Unrecognized(_) => panic!("expected a command, got Unrecognized for {input:?}"),
        }
    }

    #[test]
    fn plain_text_is_not_a_command() {
        assert!(matches!(parse_command("hello there"), ParseOutcome::NotACommand));
        assert!(matches!(parse_command(""), ParseOutcome::NotACommand));
    }

    #[test]
    fn unrecognized_slash_input_is_never_treated_as_chat() {
        assert!(matches!(parse_command("/nonsense"), ParseOutcome::Unrecognized(_)));
        assert!(matches!(parse_command("/models bogus"), ParseOutcome::Unrecognized(_)));
        assert!(matches!(parse_command("/models use"), ParseOutcome::Unrecognized(_)));
        assert!(matches!(parse_command("/"), ParseOutcome::Unrecognized(_)));
    }

    #[test]
    fn exit_and_quit_and_help_parse_with_no_args() {
        assert!(matches!(assert_recognized("/exit"), Command::Exit));
        assert!(matches!(assert_recognized("/quit"), Command::Exit));
        assert!(matches!(assert_recognized("/help"), Command::Help));
        assert!(matches!(assert_recognized("/usage"), Command::Usage));
    }

    #[test]
    fn models_subcommands_parse_their_arguments() {
        assert!(matches!(assert_recognized("/models"), Command::ModelsList));
        assert!(matches!(assert_recognized("/models reset"), Command::ModelsReset));
        assert!(matches!(assert_recognized("/models add"), Command::ModelsAdd));
        assert!(matches!(assert_recognized("/models use my-id"), Command::ModelsUse(id) if id == "my-id"));
        assert!(matches!(assert_recognized("/models edit my-id"), Command::ModelsEdit(id) if id == "my-id"));
        assert!(matches!(assert_recognized("/models remove my-id"), Command::ModelsRemove(id) if id == "my-id"));
    }

    #[test]
    fn agents_use_none_clears_while_use_with_an_id_sets_it() {
        assert!(matches!(assert_recognized("/agents use none"), Command::AgentsUse(None)));
        assert!(matches!(assert_recognized("/agents use pirata"), Command::AgentsUse(Some(id)) if id == "pirata"));
    }

    #[test]
    fn sync_subcommands_parse_their_arguments() {
        assert!(matches!(assert_recognized("/sync"), Command::SyncStatus));
        assert!(matches!(assert_recognized("/sync push"), Command::SyncPush));
        assert!(matches!(assert_recognized("/sync pull"), Command::SyncPull));
        assert!(matches!(assert_recognized("/sync pair"), Command::SyncPairShow));
    }

    #[test]
    fn sync_pair_join_captures_the_typed_code() {
        assert!(matches!(assert_recognized("/sync pair ABCD1234"), Command::SyncPairJoin(code) if code == "ABCD1234"));
    }

    #[test]
    fn sync_git_subcommands_parse() {
        assert!(matches!(assert_recognized("/sync git push"), Command::SyncGitPush));
        assert!(matches!(assert_recognized("/sync git pull"), Command::SyncGitPull));
    }

    #[test]
    fn command_matching_is_case_insensitive_on_the_head_word() {
        assert!(matches!(assert_recognized("/EXIT"), Command::Exit));
        assert!(matches!(assert_recognized("/Models"), Command::ModelsList));
    }

    #[test]
    fn parse_provider_kind_matches_the_toml_snake_case_spelling() {
        assert_eq!(parse_provider_kind("gemini"), Some(Provider::Gemini));
        assert_eq!(parse_provider_kind("OpenAI".to_lowercase().as_str()), Some(Provider::Openai));
        assert_eq!(parse_provider_kind("openai_compatible"), Some(Provider::OpenaiCompatible));
        assert_eq!(parse_provider_kind("nonsense"), None);
    }

    #[test]
    fn kind_label_round_trips_through_parse_provider_kind() {
        for kind in [Provider::Gemini, Provider::Openai, Provider::Anthropic, Provider::OpenaiCompatible] {
            assert_eq!(parse_provider_kind(kind_label(kind)), Some(kind));
        }
    }

    fn candidates_for(input: &str) -> Vec<&'static str> {
        let mut candidates = current_word(input).map(|(_, candidates)| candidates).unwrap_or_default();
        candidates.sort_unstable();
        candidates
    }

    #[test]
    fn current_word_completes_the_top_level_command_name() {
        assert_eq!(candidates_for("/mo"), vec!["models"]);
        assert_eq!(candidates_for("/e"), vec!["exit"]);
    }

    #[test]
    fn current_word_completes_a_subcommand_name() {
        assert_eq!(candidates_for("/models u"), vec!["use"]);
        assert_eq!(candidates_for("/agents "), vec!["create", "edit", "remove", "use"]);
        assert_eq!(candidates_for("/sync pu"), vec!["pull", "push"]);
        assert_eq!(candidates_for("/sync g"), vec!["git"]);
    }

    #[test]
    fn current_word_has_nothing_once_past_the_subcommand_word() {
        assert!(candidates_for("/models use my-i").is_empty());
        assert!(candidates_for("plain text").is_empty());
        assert!(candidates_for("/bogus").is_empty());
    }

    #[test]
    fn ghost_suggestion_completes_an_unambiguous_top_level_word() {
        assert_eq!(ghost_suggestion("/mo").as_deref(), Some("dels"));
        assert_eq!(ghost_suggestion("/ex").as_deref(), Some("it"));
    }

    #[test]
    fn ghost_suggestion_completes_an_unambiguous_subcommand_word() {
        assert_eq!(ghost_suggestion("/models u").as_deref(), Some("se"));
        assert_eq!(ghost_suggestion("/agents c").as_deref(), Some("reate"));
    }

    #[test]
    fn ghost_suggestion_is_none_when_ambiguous_or_already_complete_or_out_of_grammar() {
        assert_eq!(ghost_suggestion("/"), None); // "" matches every top-level command — ambiguous
        assert_eq!(ghost_suggestion("/models"), None); // already a full, unique match
        assert_eq!(ghost_suggestion("/models use my-i"), None); // past the subcommand word
        assert_eq!(ghost_suggestion("plain text"), None);
    }
}
