import { useEffect, useState } from "react";
import type { AddTabToGroupResponse, BackgroundEvent, ListGroupTabsResponse, OkResponse } from "../background/popup_protocol";
import type { GroupTab } from "../background/tab_group";

/**
 * The "Warden" tab group (P69) — lets the DOM tools (browser_read_page/click_element/navigate/
 * extract_text) act on more than the currently active tab. Adding a tab here is the user gesture
 * that grants `activeTab` for it; the AI never adds a tab on its own (see `tab_group.ts`).
 */
export default function TabsView() {
  const [tabs, setTabs] = useState<GroupTab[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  function refresh() {
    chrome.runtime.sendMessage({ type: "listGroupTabs" }).then((res: ListGroupTabsResponse) => {
      if (res.ok) {
        setTabs(res.tabs);
        setError(null);
      } else {
        setTabs((current) => current ?? []);
        setError(res.error ?? "falha ao listar as abas");
      }
    });
  }

  useEffect(() => {
    refresh();
    function onEvent(event: BackgroundEvent) {
      if (event.type === "groupChanged") refresh();
    }
    chrome.runtime.onMessage.addListener(onEvent);
    return () => chrome.runtime.onMessage.removeListener(onEvent);
  }, []);

  function handleAdd() {
    setAdding(true);
    setError(null);
    chrome.runtime.sendMessage({ type: "addTabToGroup" }).then((res: AddTabToGroupResponse) => {
      setAdding(false);
      if (res.ok) refresh();
      else setError(res.error ?? "falha ao adicionar a aba");
    });
  }

  function handleRemove(tabId: number) {
    setError(null);
    chrome.runtime.sendMessage({ type: "removeTabFromGroup", tabId }).then((res: OkResponse) => {
      if (res.ok) refresh();
      else setError(res.error ?? "falha ao remover a aba");
    });
  }

  return (
    <div className="tabs-view">
      <div className="tabs-toolbar">
        <span className="tabs-hint">
          Abas que a IA pode ler/clicar/navegar diretamente, mesmo sem estarem em foco. Sem nenhuma adicionada, as tools de
          navegador continuam agindo na aba ativa no momento, como sempre.
        </span>
        <button type="button" onClick={handleAdd} disabled={adding}>
          {adding ? "Adicionando…" : "+ Adicionar esta aba"}
        </button>
      </div>
      {error && <p className="error-banner">{error}</p>}
      {tabs === null ? (
        <p className="tabs-hint">Carregando…</p>
      ) : tabs.length === 0 ? (
        <p className="tabs-hint">Nenhuma aba adicionada ainda.</p>
      ) : (
        <ul className="tabs-list">
          {tabs.map((tab) => (
            <li key={tab.tabId} className="tabs-item">
              <div className="tabs-item-header">
                <span className="tabs-item-name">
                  {tab.title || tab.url}
                  {tab.active && " (ativa)"}
                </span>
                <button type="button" className="link-button tabs-danger" onClick={() => handleRemove(tab.tabId)}>
                  Remover
                </button>
              </div>
              <p className="tabs-item-url">{tab.url}</p>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
