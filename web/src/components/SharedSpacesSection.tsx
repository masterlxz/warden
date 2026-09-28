import { useCallback, useEffect, useState } from "react";
import { UserError, type ServerConnection } from "../hub/connection";
import type { SpaceInfo, UserInfo } from "../hub/messages";

// Shared spaces (P84 fatia 3): folders of the owner's vault that members see at
// `compartilhado/<name>/` in their own vault — reading only, or writing too. A member's turn never
// sees the rest of the owner's vault. Every change asks for the pairing key, like the people above.

const EVERYONE = "*";

type Access = "none" | "read" | "write";

type Asking =
  /** `originalId` absent: a new space. */
  | { kind: "edit"; originalId?: string; space: SpaceInfo }
  | { kind: "remove"; space: SpaceInfo };

function accessOf(space: SpaceInfo, who: string): Access {
  if (space.writers.includes(who)) return "write";
  if (space.readers.includes(who)) return "read";
  return "none";
}

function withAccess(space: SpaceInfo, who: string, access: Access): SpaceInfo {
  const readers = space.readers.filter((r) => r !== who);
  const writers = space.writers.filter((w) => w !== who);
  if (access === "read") readers.push(who);
  if (access === "write") writers.push(who);
  return { ...space, readers, writers };
}

/** Who is in a space, in a few words. */
function peopleLabel(space: SpaceInfo, users: UserInfo[]): string {
  const name = (id: string) => (id === EVERYONE ? "todo mundo" : (users.find((u) => u.id === id)?.name ?? id));
  const parts = [];
  if (space.writers.length > 0) parts.push(`escrevem: ${space.writers.map(name).join(", ")}`);
  if (space.readers.length > 0) parts.push(`leem: ${space.readers.map(name).join(", ")}`);
  return parts.length > 0 ? parts.join(" · ") : "ninguém ainda";
}

/** The folders of the owner's vault, from its files, for suggesting one. */
function foldersOf(files: string[]): string[] {
  const folders = new Set<string>();
  for (const file of files) {
    const parts = file.split("/");
    for (let i = 1; i < parts.length; i++) folders.add(parts.slice(0, i).join("/"));
  }
  return [...folders].sort();
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function SharedSpacesSection({ conn, users }: { conn: ServerConnection | null; users: UserInfo[] }) {
  const [spaces, setSpaces] = useState<SpaceInfo[] | null>(null);
  const [folders, setFolders] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pairingKey, setPairingKey] = useState("");
  const [keyError, setKeyError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!conn) return;
    try {
      setSpaces(await conn.listSpaces());
      setError(null);
    } catch (err) {
      setError(message(err));
    }
  }, [conn]);

  useEffect(() => {
    void load();
    conn
      ?.listVaultFiles()
      .then((files) => setFolders(foldersOf(files)))
      .catch(() => setFolders([]));
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
      const space = asking.space;
      setSpaces(
        asking.kind === "edit"
          ? await conn.saveSpace(pairingKey, { ...space, id: space.id.trim().toLowerCase(), folder: space.folder.trim() }, asking.originalId)
          : await conn.deleteSpace(pairingKey, space.id),
      );
      cancel();
    } catch (err) {
      setKeyError(err instanceof UserError && err.authRejected ? "Chave de pareamento errada." : message(err));
    } finally {
      setBusy(false);
    }
  }

  const keyForm = (label: string, danger: boolean, extra: React.ReactNode, ready = true) => (
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
        <button type="submit" className={danger ? "primary-button skills-danger" : "primary-button"} disabled={busy || !ready || pairingKey.trim() === "" || !conn}>
          {busy ? "Aguarde…" : label}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={cancel}>
          Cancelar
        </button>
      </div>
    </form>
  );

  const editor = (editing: Extract<Asking, { kind: "edit" }>) => {
    const space = editing.space;
    const set = (next: SpaceInfo) => setAsking({ ...editing, space: next });
    const everyone = accessOf(space, EVERYONE);
    const accessSelect = (who: string, value: Access) => (
      <select value={value} onChange={(e) => set(withAccess(space, who, e.target.value as Access))}>
        <option value="none">Não vê</option>
        <option value="read">Só lê</option>
        <option value="write">Lê e escreve</option>
      </select>
    );
    return keyForm(
      editing.originalId ? "Salvar" : "Compartilhar",
      false,
      <>
        <label className="settings-field">
          Nome
          <input autoCapitalize="none" value={space.id} onChange={(e) => set({ ...space, id: e.target.value })} placeholder="casa" autoFocus={!editing.originalId} />
          <span className="field-hint">Letras minúsculas, números, - ou _. A pessoa vê a pasta em compartilhado/{space.id.trim().toLowerCase() || "nome"}/.</span>
        </label>
        <label className="settings-field">
          Pasta do seu vault
          <input autoCapitalize="none" list="shared-space-folders" value={space.folder} onChange={(e) => set({ ...space, folder: e.target.value })} placeholder="casa" />
          <datalist id="shared-space-folders">
            {folders.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
          <span className="field-hint">Relativa à raiz do vault, como casa ou viagens/2026. Se não existir, é criada quando alguém escrever nela.</span>
        </label>
        <fieldset className="settings-tools">
          <label className="settings-check">
            Todo mundo {accessSelect(EVERYONE, everyone)}
          </label>
          {users.map((user) => (
            <label key={user.id} className="settings-check">
              {user.name} {accessSelect(user.id, accessOf(space, user.id))}
            </label>
          ))}
          <span className="field-hint">Só você cria espaços. O resto do seu vault continua só seu.</span>
        </fieldset>
      </>,
      space.id.trim() !== "" && space.folder.trim() !== "",
    );
  };

  return (
    <>
      <div className="skills-header">
        <h2>Espaços compartilhados</h2>
        <button
          type="button"
          className="primary-button"
          disabled={!conn || asking !== null || users.length === 0}
          onClick={() => setAsking({ kind: "edit", space: { id: "", folder: "", readers: [], writers: [] } })}
        >
          Compartilhar pasta
        </button>
      </div>
      <p className="skills-hint">
        Pastas do seu vault que outras pessoas veem dentro do vault delas, em <code>compartilhado/</code>. Os agentes que conversam com elas leem
        (e, se você deixar, escrevem) só nessas pastas, nunca no resto da sua memória.
      </p>
      {error && <p className="error-banner">{error}</p>}
      {asking?.kind === "edit" && !asking.originalId && editor(asking)}

      {spaces === null ? (
        <p className="skills-hint">Carregando…</p>
      ) : spaces.length === 0 ? (
        <p className="skills-hint">{users.length === 0 ? "Adicione alguém antes de compartilhar uma pasta." : "Nenhuma pasta compartilhada."}</p>
      ) : (
        <ul className="skills-list">
          {spaces.map((space) => {
            const mine = asking !== null && (asking.kind === "remove" ? asking.space.id === space.id : asking.originalId === space.id) ? asking : null;
            return (
              <li key={space.id} className="skills-item">
                <div className="skills-item-header">
                  <span className="skills-item-name">{space.id}</span>
                  <code>{space.folder}</code>
                </div>
                <p className="skills-item-description">{peopleLabel(space, users)}</p>
                {mine?.kind === "edit" && editor(mine)}
                {mine?.kind === "remove" &&
                  keyForm(
                    "Parar de compartilhar",
                    true,
                    <p className="error-banner">Quem via esta pasta deixa de ver na próxima mensagem. A pasta e as notas continuam no seu vault.</p>,
                  )}
                {!mine && (
                  <div className="skills-actions">
                    <button type="button" className="link-button" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "edit", originalId: space.id, space })}>
                      Editar
                    </button>
                    <button type="button" className="link-button skills-danger" disabled={!conn || asking !== null} onClick={() => setAsking({ kind: "remove", space })}>
                      Parar de compartilhar
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}
