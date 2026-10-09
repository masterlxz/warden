import { useCallback, useEffect, useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { NodeFolder, RemovedUser, UserInfo } from "../hub/messages";
import { ORG_ACCESS_CHOICES, orgAccessLabel, orgAccessOf, type OrgAccess } from "../hub/org";
import RecoveryPolicySection from "./RecoveryPolicySection";
import SharedSpacesSection from "./SharedSpacesSection";

// People (P84): the members of this workspace besides the owner. Each signs in with a username and
// password, and has their own vault and conversations on this hub. Every change asks for the pairing
// key, like approving a device. A provisional password is shown once, right after it's created.

type Asking =
  | { kind: "create"; id: string; name: string }
  | { kind: "rename"; user: UserInfo; name: string }
  | { kind: "reset"; user: UserInfo }
  | { kind: "remove"; user: UserInfo }
  | { kind: "restore"; user: RemovedUser }
  | { kind: "invite"; user: UserInfo }
  | { kind: "unlink"; user: UserInfo }
  /** `null`: back to the safe default. */
  | { kind: "tools"; user: UserInfo; tools: string[] | null }
  /** P115: `""` is the workspace's own model. */
  | { kind: "learning"; user: UserInfo; provider: string }
  /** P120: what they do with the organization of the agents. */
  | { kind: "orgAccess"; user: UserInfo; access: OrgAccess }
  /** P102: the folders they may work in, one per line — of the hub's machine, and on nodes as `<node id>:<path>`. */
  | { kind: "folders"; user: UserInfo; workdirs: string; nodeWorkdirs: string };

/** `node-id:path` lines → the entries; a line with no `:` is a mistake the hub would also refuse. */
function parseNodeLines(text: string): NodeFolder[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const colon = line.indexOf(":");
      return colon < 0 ? { node: line, path: "" } : { node: line.slice(0, colon).trim(), path: line.slice(colon + 1).trim() };
    });
}

const lines = (text: string): string[] => text.split("\n").map((l) => l.trim()).filter(Boolean);

/** Which folders a person may pick, in a few words. */
function foldersLabel(user: UserInfo): string {
  const count = (user.workdirs?.length ?? 0) + (user.nodeWorkdirs?.length ?? 0);
  return count === 0 ? "sem pastas de trabalho" : `${count} pasta${count === 1 ? "" : "s"} de trabalho liberada${count === 1 ? "" : "s"}`;
}

/** Mirrors `warden_bootstrap::users::default_member_tool`: what a member has when you never chose. */
function safeByDefault(tool: string): boolean {
  return ["read_file", "write_file", "use_skill", "read_skill_file", "manage_skill", "delegate_task", "jobs", "budget", "generate_document", "search_history"].includes(tool) || tool.startsWith("tavily");
}

/** Mirrors `NEVER_FOR_MEMBERS`: tools a member never gets, so they aren't offered. */
const NEVER_FOR_MEMBERS = ["delegate_to_agent", "message_agent", "manage_agents", "manage_tasks", "usage_stats"];

/** What each person's tools reach, in a few words. */
function toolsLabel(user: UserInfo): string {
  if (user.tools == null) return "ferramentas padrão (arquivos e skills do próprio vault, busca na web)";
  if (user.tools.length === 0) return "nenhuma ferramenta";
  return `ferramentas: ${user.tools.join(", ")}`;
}

/** P84 fatia 4: whether a person's data is encrypted on the hub — the owner sees the state, never the data. */
function dataLabel(user: UserInfo): string {
  if (!user.encrypted) return "dados ainda sem criptografia (a pessoa precisa entrar uma vez com a senha dela)";
  if (user.needsRecovery) return "dados criptografados; a pessoa precisa do código de recuperação dela";
  return "dados criptografados";
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function PeopleView({ conn }: { conn: ServerConnection | null }) {
  const [users, setUsers] = useState<UserInfo[] | null>(null);
  /** Removed members whose encrypted data the hub kept (P84 fatia 4). */
  const [removed, setRemoved] = useState<RemovedUser[]>([]);
  /** The workspace's recovery policy (P84 fatia 4 parte B). */
  const [recoveryPolicy, setRecoveryPolicy] = useState("private");
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** The provisional password to hand over, and to whom — shown once. */
  const [shown, setShown] = useState<{ id: string; password: string } | null>(null);
  /** An invite to link a TruthID, and for whom — shown once. */
  const [invite, setInvite] = useState<{ id: string; code: string } | null>(null);
  /** The hub's tools, for choosing each person's. */
  const [toolNames, setToolNames] = useState<string[]>([]);
  /** The hub's models and combos, for choosing the one each person's learning uses. */
  const [modelIds, setModelIds] = useState<string[]>([]);

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      const list = await conn.listUsers();
      setUsers(list.users);
      setRemoved(list.removed);
      setRecoveryPolicy(list.recoveryPolicy ?? "private");
      setError(null);
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
    conn
      ?.requestSettings()
      .then(({ settings }) => {
        setToolNames(settings.toolNames.filter((t) => !NEVER_FOR_MEMBERS.includes(t)));
        setModelIds([...settings.providers.map((p) => p.id), ...settings.combos.map((c) => c.id)]);
      })
      .catch(() => {
        setToolNames([]);
        setModelIds([]);
      });
  }, [conn, load]);

  function cancel() {
    setAsking(null);
    setPairingKey("");
    setKeyError(null);
  }

  async function confirm() {
    if (!conn || !asking) return;
    setBusy(true);
    setKeyError(null);
    try {
      const reply =
        asking.kind === "create"
          ? await conn.saveUser(pairingKey, asking.id.trim().toLowerCase(), asking.name.trim(), true)
          : asking.kind === "rename"
            ? await conn.saveUser(pairingKey, asking.user.id, asking.name.trim(), false)
            : asking.kind === "reset"
              ? await conn.resetPassword(pairingKey, asking.user.id)
              : asking.kind === "tools"
                ? await conn.setUserTools(pairingKey, asking.user.id, asking.tools)
                : asking.kind === "learning"
                  ? await conn.setUserLearningProvider(pairingKey, asking.user.id, asking.provider || null)
                : asking.kind === "orgAccess"
                  ? await conn.setUserOrgAccess(pairingKey, asking.user.id, asking.access)
                : asking.kind === "folders"
                  ? await conn.setUserWorkdirs(pairingKey, asking.user.id, lines(asking.workdirs), parseNodeLines(asking.nodeWorkdirs))
                : asking.kind === "restore"
                  ? await conn.restoreUser(pairingKey, asking.user.id)
                  : asking.kind === "invite"
                  ? await conn.createInvite(pairingKey, asking.user.id)
                  : asking.kind === "unlink"
                    ? await conn.unlinkTruthId(pairingKey, asking.user.id)
                    : await conn.removeUser(pairingKey, asking.user.id);
      setUsers(reply.users);
      setRemoved(reply.removed);
      if (reply.inviteCode && asking.kind === "invite") setInvite({ id: asking.user.id, code: reply.inviteCode });
      if (reply.tempPassword) {
        setShown({ id: asking.kind === "create" ? asking.id.trim().toLowerCase() : asking.kind === "reset" ? asking.user.id : "", password: reply.tempPassword });
      }
      cancel();
    } catch (err) {
      if (err instanceof UserError && err.authRejected) {
        setKeyError("Chave de pareamento errada.");
      } else {
        setKeyError(message(err));
      }
    } finally {
      setBusy(false);
    }
  }

  const keyForm = (label: string, danger = false, extra?: React.ReactNode) => (
    <form
      className="settings-confirm devices-confirm"
      onSubmit={(e) => {
        e.preventDefault();
        void confirm();
      }}
    >
      {extra}
      <label className="settings-field">
        Chave de pareamento do hub
        <input type="password" autoComplete="current-password" value={pairingKey} onChange={(e) => setPairingKey(e.target.value)} />
        <span className="field-hint">É pedida a cada mudança.</span>
      </label>
      {keyError && <p className="error-banner">{keyError}</p>}
      <div className="skills-actions">
        <button type="submit" className={danger ? "primary-button skills-danger" : "primary-button"} disabled={busy || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : label}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancel}>
          Cancelar
        </button>
      </div>
    </form>
  );

  return (
    <div className="skills-view">
      <div className="skills-header">
        <h2>Pessoas</h2>
        <button type="button" className="primary-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "create", id: "", name: "" })}>
          Adicionar pessoa
        </button>
      </div>
      <p className="skills-hint">
        Quem mais usa este Warden. Cada pessoa entra com o próprio usuário e senha e tem o próprio vault e as próprias conversas, que você não vê.
        Ela conversa com os agentes que você compartilhar (em Configurações), sempre com a memória dela e só com as ferramentas que você
        liberar aqui, e pode criar agentes próprios.
      </p>
      {error && <p className="error-banner">{error}</p>}

      {shown && (
        <div className="settings-confirm">
          <p>
            Senha provisória de <strong>{shown.id}</strong> (só aparece agora): <code>{shown.password}</code>
          </p>
          <p className="skills-hint">Passe para a pessoa junto com o usuário. No primeiro acesso ela troca pela senha dela.</p>
          <button type="button" className="link-button" onClick={() => setShown(null)}>
            Já anotei
          </button>
        </div>
      )}

      {invite && (
        <div className="settings-confirm">
          <p>
            Convite de TruthID para <strong>{invite.id}</strong> (só aparece agora, vale 7 dias e serve uma vez): <code>{invite.code}</code>
          </p>
          <p className="skills-hint">A pessoa entra, abre "Trocar senha" no cabeçalho e, em "Ligar meu TruthID", informa este código e o usuário dela no TruthID.</p>
          <button type="button" className="link-button" onClick={() => setInvite(null)}>
            Já anotei
          </button>
        </div>
      )}

      {asking?.kind === "create" &&
        keyForm(
          "Criar",
          false,
          <>
            <label className="settings-field">
              Usuário
              <input
                autoCapitalize="none"
                value={asking.id}
                onChange={(e) => setAsking({ ...asking, id: e.target.value })}
                placeholder="ana"
                autoFocus
              />
              <span className="field-hint">Letras minúsculas, números, - ou _. É como a pessoa entra.</span>
            </label>
            <label className="settings-field">
              Nome
              <input value={asking.name} onChange={(e) => setAsking({ ...asking, name: e.target.value })} placeholder="Ana Souza" />
            </label>
          </>,
        )}

      {users === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : users.length === 0 ? (
        <p className="skills-hint">Só você por enquanto.</p>
      ) : (
        <ul className="skills-list">
          {users.map((user) => {
            const mine = asking !== null && asking.kind !== "create" && asking.user.id === user.id ? asking : null;
            return (
              <li key={user.id} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">{user.name}</span>
                  <span className={`devices-status devices-status--${user.mustChangePassword ? "pending" : "approved"}`}>
                    {user.mustChangePassword ? "Senha provisória" : "Ativo"}
                  </span>
                </div>
                <p className="skills-item-description">
                  <code>{user.id}</code> · {toolsLabel(user)}
                  {user.agents.length > 0 && ` · agentes próprios: ${user.agents.join(", ")}`}
                  {` · ${dataLabel(user)}`}
                  {` · ${foldersLabel(user)}`}
                  {user.learningProvider ? ` · aprendizado com: ${user.learningProvider}` : ""}
                  {` · ${orgAccessLabel(orgAccessOf(user.orgAccess))}`}
                  {user.truthid ? ` · TruthID: @${user.truthid}` : user.inviteOpen ? " · convite de TruthID aberto" : ""}
                </p>
                {mine?.kind === "rename" &&
                  keyForm(
                    "Salvar",
                    false,
                    <label className="settings-field">
                      Nome
                      <input value={mine.name} onChange={(e) => setAsking({ ...mine, name: e.target.value })} autoFocus />
                    </label>,
                  )}
                {mine?.kind === "reset" && keyForm("Gerar senha provisória")}
                {mine?.kind === "invite" && keyForm("Gerar convite")}
                {mine?.kind === "unlink" && keyForm("Desligar TruthID", true)}
                {mine?.kind === "tools" &&
                  keyForm(
                    "Salvar ferramentas",
                    false,
                    <fieldset className="settings-tools">
                      <label className="settings-check">
                        <input type="checkbox" checked={mine.tools === null} onChange={(e) => setAsking({ ...mine, tools: e.target.checked ? null : toolNames.filter(safeByDefault) })} />
                        Usar o padrão seguro
                      </label>
                      {mine.tools !== null &&
                        toolNames.map((tool) => (
                          <label key={tool} className="settings-check">
                            <input
                              type="checkbox"
                              checked={mine.tools!.includes(tool)}
                              onChange={(e) => setAsking({ ...mine, tools: e.target.checked ? [...mine.tools!, tool] : mine.tools!.filter((t) => t !== tool) })}
                            />
                            <code>{tool}</code>
                            {!safeByDefault(tool) && <span className="skills-danger"> alcança o que é seu (terminal, nós, integrações)</span>}
                          </label>
                        ))}
                      <span className="field-hint">Um agente nunca passa disso, nem do que o próprio agente pode.</span>
                    </fieldset>,
                  )}
                {mine?.kind === "folders" &&
                  keyForm(
                    "Salvar pastas",
                    false,
                    <>
                      <label className="settings-field">
                        Pastas do computador do hub que {user.name} pode escolher
                        <textarea
                          rows={3}
                          value={mine.workdirs}
                          onChange={(e) => setAsking({ ...mine, workdirs: e.target.value })}
                          placeholder="/srv/trabalho"
                          autoFocus
                        />
                        <span className="field-hint">Uma por linha, caminho absoluto. Vale a pasta e tudo dentro dela. Vazio: nenhuma.</span>
                      </label>
                      <label className="settings-field">
                        Pastas de nós
                        <textarea
                          rows={3}
                          value={mine.nodeWorkdirs}
                          onChange={(e) => setAsking({ ...mine, nodeWorkdirs: e.target.value })}
                          placeholder="node-casa-1a2b3c4d:projetos"
                        />
                        <span className="field-hint">
                          Uma por linha, no formato <code>id-do-nó:pasta</code>, com a pasta relativa ao que o nó empresta (só o id, sem a pasta, libera tudo o que ele empresta). O id aparece
                          em Aparelhos, nos nós.
                        </span>
                      </label>
                      <span className="field-hint">
                        {user.name} escolhe a pasta antes da primeira mensagem de uma conversa. Com elas a IA lê e escreve só ali; o terminal só se você liberar a ferramenta shell.
                      </span>
                    </>,
                  )}
                {mine?.kind === "learning" &&
                  keyForm(
                    "Salvar modelo",
                    false,
                    <label className="settings-field">
                      Modelo com que a IA aprende das conversas de {user.name}
                      <select value={mine.provider} onChange={(e) => setAsking({ ...mine, provider: e.target.value })}>
                        <option value="">Padrão do workspace</option>
                        {/* A model the hub no longer has stays listed, so saving doesn't silently swap it. */}
                        {mine.provider && !modelIds.includes(mine.provider) && <option value={mine.provider}>{mine.provider} (não existe mais)</option>}
                        {modelIds.map((id) => (
                          <option key={id} value={id}>
                            {id}
                          </option>
                        ))}
                      </select>
                      <span className="field-hint">O gasto conta no canal “learning”. Vale só se o aprendizado estiver ligado e a pessoa não tiver optado por sair.</span>
                    </label>,
                  )}
                {mine?.kind === "orgAccess" &&
                  keyForm(
                    "Salvar acesso",
                    false,
                    <fieldset className="settings-field">
                      <legend>O que {user.name} faz com o organograma dos agentes</legend>
                      {ORG_ACCESS_CHOICES.map((choice) => (
                        <label key={choice.value} className="settings-check">
                          <input type="radio" name={`org-${user.id}`} checked={mine.access === choice.value} onChange={() => setAsking({ ...mine, access: choice.value })} />
                          {choice.label}
                          <span className="field-hint"> {choice.hint}</span>
                        </label>
                      ))}
                      <span className="field-hint">A árvore é uma só, a do workspace. Com “Vê e edita”, quem tem acesso muda a hierarquia sem a chave do hub; os poderes de cada agente continuam só seus.</span>
                    </fieldset>,
                  )}
                {mine?.kind === "remove" &&
                  keyForm(
                    "Remover",
                    true,
                    <p className="error-banner">
                      {user.name} sai do workspace e os aparelhos são desconectados.{" "}
                      {user.encrypted
                        ? "O vault e as conversas ficam no disco do hub, criptografados. A pessoa fica guardada como removida, com a chave que abre os dados: warden-server users restore traz de volta, e users purge apaga tudo de vez."
                        : "O vault e as conversas ficam guardados no hub."}
                    </p>,
                  )}
                {!mine && (
                  <div className="skills-actions">
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "rename", user, name: user.name })}>
                      Renomear
                    </button>
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "tools", user, tools: user.tools ?? null })}>
                      Ferramentas
                    </button>
                    <button
                      type="button"
                      className="link-button"
                      disabled={!conn || asking !== null}
                      onClick={() =>
                        setAsking({
                          kind: "folders",
                          user,
                          workdirs: (user.workdirs ?? []).join("\n"),
                          nodeWorkdirs: (user.nodeWorkdirs ?? []).map((f) => `${f.node}:${f.path}`).join("\n"),
                        })
                      }
                    >
                      Pastas de trabalho
                    </button>
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "learning", user, provider: user.learningProvider ?? "" })}>
                      Modelo do aprendizado
                    </button>
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "orgAccess", user, access: orgAccessOf(user.orgAccess) })}>
                      Organograma
                    </button>
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "reset", user })}>
                      Nova senha provisória
                    </button>
                    {user.truthid ? (
                      <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "unlink", user })}>
                        Desligar TruthID
                      </button>
                    ) : (
                      <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "invite", user })}>
                        Convidar para o TruthID
                      </button>
                    )}
                    {!user.truthid && user.inviteOpen && (
                      <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "unlink", user })}>
                        Cancelar convite
                      </button>
                    )}
                    <button type="button" className="link-button skills-danger" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "remove", user })}>
                      Remover
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {removed.length > 0 && (
        <>
          <h3>Pessoas removidas</h3>
          <p className="skills-hint">
            Saíram do workspace, mas os dados criptografados delas continuam no disco do hub, com a chave que os abre guardada. Restaurar traz a
            pessoa de volta com a senha que ela tinha; os aparelhos dela foram desconectados na remoção, então ela entra de novo. Apagar de vez é só
            pela linha de comando: <code>warden-server users purge</code>.
          </p>
          <ul className="skills-list">
            {removed.map((gone) => (
              <li key={gone.id} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">{gone.name}</span>
                  <span className="devices-status devices-status--pending">Removida</span>
                </div>
                <p className="skills-item-description">
                  <code>{gone.id}</code>
                </p>
                {asking?.kind === "restore" && asking.user.id === gone.id ? (
                  keyForm("Restaurar")
                ) : (
                  <div className="skills-actions">
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "restore", user: gone })}>
                      Restaurar
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        </>
      )}

      <SharedSpacesSection conn={conn} users={users ?? []} />

      <RecoveryPolicySection
        conn={conn}
        users={users ?? []}
        policy={recoveryPolicy}
        onChanged={(next, policy, temp) => {
          if (next) setUsers(next);
          setRecoveryPolicy(policy);
          if (temp) setShown(temp);
        }}
      />
    </div>
  );
}
