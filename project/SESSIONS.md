# Log de Sessões

> **Nota**: Este log foi criado junto com o projeto. As sessões serão registradas aqui conforme o trabalho avança.
>
> Última atualização: 2026-09-08 (Sessão 57)

---

### 2026-09-08 — Sessão 57 (continuação 7)

- **Objetivo**: usuário trouxe uma spec pronta na raiz do repo (`spec-storage-provider-agente.md`)
  propondo desacoplar onde a memória do agente é armazenada de qual identidade/pagamento autoriza
  isso — pediu pra ler, incorporar tudo no `project/`, apagar o arquivo da raiz e commitar.

**O que foi feito**:

- Lido `spec-storage-provider-agente.md` na íntegra. Confirmado que ele generaliza o que já existe
  (`warden-sync`/P37, TruthID como pagador Arweave via `pin()`) atrás de duas interfaces novas no
  core (`StorageProvider`: read/write/list/delete/exportAll/importAll; `AuthProvider`:
  getUserId/isSubscriptionActive/login/logout), com TruthID virando **plugin opcional**
  (`DecentralizedVaultProvider`) entre 4 implementações propostas (`LocalFSProvider`,
  `RemoteNodeProvider`, `ManagedCloudProvider`, `DecentralizedVaultProvider`), fluxo de migração
  entre providers, e um escopo de MVP sugerido mas explicitamente marcado como "a decidir com
  Fabio antes de começar a implementação".
- Registrado como **P61** em `PENDING.md` (Decisões em Aberto) — conteúdo completo da spec
  incorporado no corpo da pendência, cross-referenciando P37/P24 (o `DecentralizedVaultProvider`
  proposto é essencialmente uma generalização do `warden-sync` já implementado).
- Adicionada seção nova "Storage Provider plugável (desacoplar vault de TruthID)" em
  `ROADMAP.md`, junto de "Plugin system" (seção conceitualmente mais próxima), apontando pro
  detalhe completo em P61.
- Apagado `spec-storage-provider-agente.md` da raiz — conteúdo já vive em `project/`.
- Nenhuma decisão de arquitetura foi tomada nesta sessão (a spec pede confirmação de escopo com o
  usuário antes de codar) — só a incorporação ao sistema de planejamento do projeto.

**Próximo passo**: confirmar com o usuário o escopo de MVP sugerido na spec (v1 =
`LocalFSProvider` + `DecentralizedVaultProvider`, v2 = `RemoteNodeProvider`, v3 =
`ManagedCloudProvider`) antes de começar a implementação de P61, e decidir se `warden-sync`/P37 é
refatorado por trás de `StorageProvider` ou se essa abstração nasce em paralelo.

---

### 2026-09-08 — Sessão 57 (continuação 6)

- **Objetivo**: usuário disse "pode seguir primeiro" (deixando o push do commit anterior pra
  depois). Perguntado por onde seguir dentro do P46, escolhido **`delegate_to_agent`**: uma tool
  que deixa um agente endereçar um agente **configurado** específico por id (persona/provider
  próprios), em vez de só uma tarefa anônima (`delegate_task`). Confirmado com o usuário: opt-in
  por agente (`AgentConfig.can_delegate_to_agents`), não "todo agente pode" nem "só sem agente
  selecionado". Planejado em modo formal (`/plan`) — no meio do design, achado que mudou o escopo
  real: existe **um único `Orchestrator` compartilhado** por todas as conversas (tools fixas desde
  `bootstrap()`, só persona/modelo trocam por turno), então o opt-in de verdade exigiria mexer em
  cada canal que resolve `agent_id`, não só registrar a tool em algum lugar central. Apresentado
  esse achado ao usuário com 3 caminhos (opt-in de verdade tocando desktop+CLI / inverter o eixo
  pro alvo / simplificar pra "todo agente pode") — escolhido **opt-in de verdade**, aceitando o
  custo maior.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`):

- **`crates/warden-core/src/tool/delegate_to_agent.rs` novo** — `NamedSubAgent`
  (id/description/orchestrator já com `with_model` aplicado/persona) + `DelegateToAgentTool`
  (`delegate_to_agent`, despacha por `agent_id`, erro claro em id desconhecido/argumento
  faltando). `warden-core` continua sem conhecer `AgentConfig` — recebe a lista já resolvida de
  fora. 5 testes novos (falta de argumento, despacho por id, erro em id desconhecido, persona
  chega de verdade como mensagem de sistema, spec lista todo agente).
- **`Orchestrator::with_tool` novo** (`orchestrator/mod.rs`), mesmo padrão exato de `with_model`
  (clone + registra uma tool a mais) — é o que permite anexar `delegate_to_agent` a um turno
  específico sem tocar a instância compartilhada que toda outra conversa usa.
- **`AgentConfig.can_delegate_to_agents: bool` novo** (`#[serde(default)]`, retrocompatível).
  Ajustados os 4 pontos que constroem `AgentConfig` literalmente (fixture/helper de teste do
  `warden-bootstrap`; `desktop::save_settings`, que reconstrói `agents` inteiro do payload da UI —
  precisou mover o carregamento de `existing` pro topo da função, antes só acontecia depois do
  loop de agentes; CLI `wizard_agents_create`/`wizard_agents_edit`) pra não perder o valor
  hand-edited no `config.toml` a cada save, mesmo cuidado já dado a `telegram_bot_token`/
  `delegate_max_depth`.
- **`warden_bootstrap::build_delegate_to_agent_tool(config, orchestrator)` novo** — monta a lista
  de `NamedSubAgent` a partir de `config.agents`, reaproveitando o `orchestrator` do turno como
  base de cada alvo (herda base_tools/profundidade de delegação de graça); pula com aviso (não
  fatal) um agente cujo `provider_id` não resolve.
- **Fiação nos dois únicos canais que já resolvem `agent_id` por turno**: `desktop/src-tauri/src/
  lib.rs::send_message` (dentro do `if let Some(id) = &agent_id`, confere `can_delegate_to_agents`
  e anexa a tool) e `crates/warden-cli/src/interactive.rs` (`TurnContext` ganhou um terceiro
  elemento, `resolve_turn_context` ganhou um parâmetro `orchestrator: &Orchestrator`, `run_turn`
  ganhou `extra_tool: Option<Arc<dyn Tool>>`). Telegram/WhatsApp/`warden-server` não têm suporte a
  agente nomeado nenhum hoje — nada mudou neles.
- **Limitação aceita e documentada**: um agente invocado como *alvo* de `delegate_to_agent` nunca
  ganha a tool ele mesmo, mesmo com `can_delegate_to_agents: true` — a flag só é consultada pro
  agente ativo da conversa, nunca pra um alvo. Evita cadeia chefe-de-chefe descontrolada sem
  precisar de outro limite de profundidade.
- `cargo test -p warden-core -p warden-bootstrap -p warden-cli` (70+36+34+5 testes, nenhum
  quebrado), `cargo check --workspace`/`cargo clippy --workspace --all-targets` e `npm run build`
  (tsc+vite, zero mudança de TS esperada) limpos.
- Atualizados `ARCHITECTURE.md` (entrada nova), `PENDING.md` (P46 — mais uma fatia; segue em
  aberto UI/CLI pra ligar a flag, fila de jobs, custo, isolamento), `ROADMAP.md`.

**Próximo passo**: dentro do P46, seguem em aberto UI/CLI pra ligar `can_delegate_to_agents`, fila
de jobs, controle de custo, isolamento de tools por sub-agente. Fora do P46: Fase 7.6, P8,
P47-P51, P59/P60. Push do commit anterior (profundidade configurável) e deste ainda pendente.

---

### 2026-09-08 — Sessão 57 (continuação 5)

- **Objetivo**: usuário disse "bora continuar no 46 ent". Com o núcleo recursivo já feito
  (continuação 4), oferecidas 3 fatias pra seguir dentro do P46 — usuário escolheu a menor:
  **tirar `DELEGATE_MAX_DEPTH` de constante fixa e deixar configurável** via `config.toml`/env,
  sem UI nova. Planejado em modo formal (`/plan`, 1 agente Explore mapeando agentes nomeados e o
  wiring do `DelegateTool`, usado pra confirmar que essa era mesmo a fatia certa antes de fechar
  escopo com o usuário) antes de codar.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`):

- **`FileConfig.delegate_max_depth: Option<u32>` novo** (`crates/warden-bootstrap/src/lib.rs`) —
  mesmo padrão de `enable_shell`/`Option<bool>` (serde já trata `Option` ausente como `None`, sem
  precisar de `#[serde(default)]`).
- **`resolve_delegate_max_depth(from_env, from_file) -> u32` novo**, ao lado de
  `resolve_flag`/`resolve_secret` — mesma precedência env-vence-arquivo, mesma permissividade (uma
  env mal formada cai pro arquivo/default em vez de derrubar `bootstrap()` inteiro). Constante
  antiga `DELEGATE_MAX_DEPTH` renomeada pra `DEFAULT_DELEGATE_MAX_DEPTH` (mesmo valor, `2`) — vira
  só o fallback, não mais o único valor possível. `bootstrap()` resolve
  `WARDEN_DELEGATE_MAX_DEPTH`/`config.delegate_max_depth` antes de chamar
  `build_delegating_orchestrator`.
- **Sem clamp de teto** — decisão deliberada: é exatamente esse número que o usuário pediu pra
  poder ajustar; documentar o risco (já registrado em P60) é a resposta, não capar silenciosamente
  o valor configurado.
- **Desktop (`desktop/src-tauri/src/lib.rs::save_settings`)** — como não existe UI pra esse campo,
  o save de Settings carrega `existing.delegate_max_depth` adiante em vez de zerar, mesmo
  tratamento já dado a `telegram_bot_token` (campo sem UI, hand-editable via `config.toml`).
- Teste novo (`resolve_delegate_max_depth_prefers_env_over_file`) e o fixture de
  `save_config_round_trips_through_load_config` atualizado (literal de `FileConfig` lista todo
  campo nomeado, precisou do valor novo). `cargo test -p warden-bootstrap` (36 testes),
  `cargo check --workspace` e `cargo clippy --workspace --all-targets` limpos.
- Atualizados `PENDING.md` (P46 — nota nova; P60 — risco agora ajustável pelo usuário, não
  eliminado), `ROADMAP.md` (linha desatualizada corrigida).

**Próximo passo**: dentro do P46, seguem em aberto os dois modos em si (UI/config pra "chefe" vs.
"funcionários" — candidato mapeado na exploração desta sessão: uma tool `delegate_to_agent` que
endereça um `AgentConfig` específico por id, já que hoje `DelegateTool` só sabe delegar tarefas
anônimas), fila de jobs, controle de custo, isolamento de tools. Fora do P46: Fase 7.6, P8,
P47-P51, P59/P60.

---

### 2026-09-08 — Sessão 57 (continuação 4)

- **Objetivo**: usuário disse "bora pro p46 ent". Como o ROADMAP deixava a arquitetura do P46
  explicitamente em aberto (dois modos + fila de jobs + custo + isolamento + critério de parada,
  vários eixos independentes), oferecidas 3 fatias de escopo antes de codar — usuário escolheu **só
  o núcleo técnico**: delegação recursiva (agentes que delegam pra sub-agentes que também podem
  delegar), sem UI nova nem fila de jobs/custo ainda.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, seção "Sub-agentes: Invocação Leve
vs. Autônomos"):

- **`DelegateTool` (`crates/warden-core/src/tool/delegate.rs`)** — removida a trava original ("o
  orchestrator passado pro `new` nunca pode ter outro `DelegateTool` registrado"); doc comment
  reescrito explicando que agora suporta uma cadeia de qualquer profundidade, e que o critério de
  parada é estrutural (quem monta a cadeia para de registrar `delegate_task` em algum nível), não
  uma checagem em runtime dentro da tool.
- **`warden_bootstrap::build_delegating_orchestrator` novo** (função recursiva) substitui a
  construção anterior de dois orchestrators fixos (`sub_orchestrator` + principal) — monta uma
  cadeia de até `DELEGATE_MAX_DEPTH = 2` níveis (raiz → nível-1, ainda pode delegar de novo →
  folha, terminal). Constante fixa, não configurável ainda: sem fila de jobs/controle de custo, o
  pior caso é `MAX_TOOL_ITERATIONS ^ depth` chamadas de modelo (64 nesta profundidade) se toda
  iteração em todo nível delegar.
- **Teste novo em `delegate.rs`** (`supports_bounded_recursive_delegation`) — cadeia de 3
  orchestrators compartilhando um único `ModelProvider` mockado, roteirizado por ordem de chamada
  (determinístico: cada `chat_stream` bloqueia em qualquer delegação aninhada antes da próxima
  chamada acontecer, mesmo padrão do wiring real). Prova as duas partes do contrato: o nível 1
  recebe `delegate_task` de verdade (recursão genuína) e a folha nunca recebe a tool (critério de
  parada estrutural conferido, não só assumido).
- `cargo test -p warden-core -p warden-bootstrap` (65 + 35 testes, nenhum quebrado) e `cargo
  clippy --workspace --all-targets` limpos.
- **Sem teste de ponta a ponta com um modelo real** — forçar um modelo de verdade a decidir delegar
  duas vezes de propósito não é confiável de scriptar; mesma limitação que a v1 do `DelegateTool`
  (Sessão 11) já tinha aceitado. Registrado como P60, junto com o risco de custo sem teto (acima).
- Atualizados `PENDING.md` (P46 — núcleo marcado como feito, o que segue em aberto listado
  explicitamente; P60 novo), `ARCHITECTURE.md` (entrada nova na seção de sub-agentes), `ROADMAP.md`
  (as duas seções que mencionavam P46 — "Orquestração de agentes" e "Sub-agentes autônomos" —
  atualizadas com o que já saiu do papel).

**Próximo passo**: dentro do próprio P46, faltam os dois modos em si (UI/config pra "chefe" vs.
"funcionários") e o resto do pacote (fila de jobs, custo, isolamento, profundidade configurável).
Fora do P46: Fase 7.6, P8, P47-P51, P59/P60.

---

### 2026-09-08 — Sessão 57 (continuação 3)

- **Objetivo**: usuário disse "bora continuar?" de novo (sem item travado desde o fim da parte 2 do
  P52). Oferecidas as frentes em aberto (P8, Fase 7.6, P46-P51, P58) — usuário escolheu **P58**
  (identidade visual do mobile), pedindo explicitamente pra fazer **sem esperar teste no celular
  real antes**, deixando essa verificação como pendência separada. Escopo fechado com uma pergunta
  antes de codar: só alinhar as cores de marca (vs. revisão visual mais ampla de layout/espaçamento/
  tipografia) — escolhida a opção menor.

**O que foi feito**:

- **`mobile/lib/main.dart`** — `MaterialApp` ganhou `theme`/`darkTheme`/`themeMode:
  ThemeMode.system` no lugar do `theme` único de antes; `ColorScheme.fromSeed` passou a usar os
  mesmos tokens de `desktop/src/App.css` (`--color-accent: #7c3aed` claro, `#a78bfa` escuro) em vez
  do `Colors.deepPurple` genérico do Material. Antes o app não tinha suporte a modo escuro nenhum —
  ganhou de graça alinhando com o padrão do desktop (que já segue `prefers-color-scheme`).
- Conferido que nenhuma outra mudança era necessária: as telas (`ConnectionScreen`/`ChatScreen`/
  `SyncScreen`) já usam `Theme.of(context).colorScheme` em vez de cor hardcoded, então herdam a
  marca automaticamente. As poucas cores literais que existem (`Colors.grey/orange/green/red` em
  `connection_screen.dart`) são indicadores semânticos de status de conexão, não cor de marca —
  deixadas como estavam, fora do escopo confirmado.
- `flutter analyze`/`flutter test` (33 testes, nenhum novo — mudança é só configuração de tema, sem
  lógica testável) limpos.
- Atualizado `PENDING.md`: P58 movida pra "Resolvidas"; **P59 nova** — cores de marca nunca vistas
  numa janela/emulador real, fica como pendência de verificação visual pro usuário confirmar quando
  testar no celular dele.

**Próximo passo**: seguem em aberto Fase 7.6 (build/deploy mobile), P8 (polish do CLI), P46-P51
(backlog), e agora também P59 (verificar visualmente a cor nova no celular).

---

### 2026-09-08 — Sessão 57

- **Objetivo**: usuário disse "bora continuar?". Sem próximo passo travado (Sessão 56 tinha
  terminado com o usuário pedindo só pra anotar o backlog, sem escolher item — P58). Perguntado
  por onde seguir: escolhido um item do backlog P45-P52, especificamente **P45 — um agente por
  conversa, escolhido na criação, travado depois**. Escopo confirmado com o usuário antes de
  planejar (2 perguntas): só desktop (não Telegram/WhatsApp/mobile/CLI) e agente obrigatório (sem
  mais opção "sem agente" numa conversa nova). Planejado em modo formal (`/plan`, 1 agente Explore
  mapeando o seletor atual + fluxo de criação de conversa) antes de codar.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, três entradas novas "Um agente por
conversa (P45)"):

- **`desktop/src/components/ChatArea.tsx`** — `needsAgentPick = !hasMessages && !selectedAgentId`
  (nenhum estado novo, só reaproveita `hasMessages` e o reset de `selectedAgentId` que já
  existiam desde a Sessão 43) substitui o `<select aria-label="Agent">` do `chat-header` por um
  `.agent-picker` (cards clicáveis por `AgentEntry`, ou direcionamento pra Settings se
  `agents.length === 0`) enquanto o agente não foi escolhido, e por um rótulo somente-leitura
  (`.chat-header-label`) depois — sem jeito de trocar. `MessageInput` ganhou
  `disabled={isSending || needsAgentPick}`. Seletor de modelo/provider intocado, continua editável
  a qualquer momento.
- `App.tsx` só ganhou uma prop nova (`onOpenSettings`) repassada pro `ChatArea` — nenhuma mudança
  de lógica; `handleSelectAgent`/`appendMessage`/`handleSendMessage` continuam exatamente como
  estavam, a trava é inteiramente responsabilidade do `ChatArea` não oferecer a troca.
- `SettingsView.tsx` — corrigido de passagem um texto desatualizado ("conversations use no persona
  by default"), que não fazia mais sentido com o agente virando obrigatório.
- Nenhuma mudança em `warden-bootstrap`/Rust — `Conversation.agent_id` já era um campo por-conversa
  desde a Sessão 43, só faltava parar de oferecer troca na UI.
- **Verificado via Playwright contra o dev server real** (`npm run dev`, não harness estático) —
  `window.__TAURI_INTERNALS__.invoke` mockado via `addInitScript`, usando o agente real já
  cadastrado no `config.toml` do usuário ("pirata"): picker aparecendo numa conversa nova,
  escolha do agente liberando o composer, header virando rótulo fixo depois da primeira mensagem
  (conferido tanto antes quanto depois de enviar), caso `agents.length === 0` direcionando pra
  Settings (botão testado, navegação confirmada), e claro/escuro. Um artefato do teste (duas
  mensagens enviadas ao simular Enter via `keyboard.press`) foi isolado e descartado como bug —
  confirmado com um clique real no botão de enviar que só 1 mensagem é mandada; não é um problema
  do app nem desta mudança.
- `npm run build` (tsc+vite) e `cargo check --workspace` limpos.
- Atualizados `PHASE.md` (nova entrada em Fase 6), `PENDING.md` (P45 resolvida), `ROADMAP.md`
  (item marcado como feito).

**Próximo passo**: nenhum item específico escolhido ainda — seguem em aberto Fase 7.6 (build/
deploy mobile), P8 (polish do CLI), o resto do backlog P46-P52, e P58 (identidade visual do
mobile, aguardando o usuário testar no celular real dele antes de mexer no visual).

---

### 2026-09-08 — Sessão 57 (continuação)

- **Objetivo**: usuário disse "bora seguir então". Perguntado por onde seguir de novo (sem sinal
  de prioridade): escolhido **P52 — estrutura padrão do vault + visualização pela interface**,
  especificamente a **parte 1 (estrutura fixa)**, deixando a parte 2 (UI) pra depois. Escopo
  fechado com o usuário antes de planejar: conteúdo fixo = perfil do usuário + comportamento da
  IA + feedback/lições aprendidas (as 3 opções oferecidas, todas escolhidas); arquivos na raiz do
  vault com prefixo `_`; template inicial com uma linha de orientação curta (não em branco).
  Planejado em modo formal (`/plan`, 1 agente Explore mapeando como o vault entra no contexto do
  modelo hoje e onde inicializar os arquivos) antes de codar.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, cinco entradas novas "Estrutura
fixa do vault (P52, parte 1)"):

- **`_profile.md`/`_behavior.md`/`_feedback.md`** — 3 arquivos reservados na raiz do vault
  (`FIXED_VAULT_FILES`, `crates/warden-core/src/memory/mod.rs`), visíveis (não dot-prefixed, ao
  contrário de `.warden/`) — sincronizam via `warden-sync` sem nenhuma mudança nele.
- **`Vault::standing_memory()` novo** — lê os 3 arquivos, monta um bloco único pulando seção
  vazia/ausente. **`collect_markdown_files`** passou a excluir os 3 (só quando na raiz — um
  `notes/_profile.md` do usuário continua pesquisável normalmente) de `search`/`search_semantic`,
  evitando duplicar o conteúdo já injetado fixo e evitando que consumam o orçamento de 8 hits da
  busca livre. `list_all_files` (sync) não é tocada.
- **`Orchestrator::handle_turn_streaming`** injeta o bloco como mensagem de sistema sempre que
  não-vazio, entre a persona e a busca por relevância (ordem final: persona → memória fixa → busca
  → histórico → turno atual). Por estar no único ponto real de implementação, todos os canais
  (CLI, desktop, Telegram, WhatsApp, mobile) herdam de graça, sem tocar em nenhum deles.
  `seed_default_vault_files` novo em `warden-bootstrap`, chamado logo após `Vault::new(vault_path)`
  em `bootstrap()` — idempotente (só escreve se o arquivo ainda não existe), então um vault
  restaurado via `warden-sync` de outro device não é tocado.
- **Verificado de ponta a ponta com o Gemini real** (via `warden-cli`, chave já configurada) —
  harness em pty Python (mesma técnica de sessões anteriores) editando `_profile.md` com um fato
  fictício ("tenho um dragão de estimação chamado Fumaça") sem nunca mencionar isso na conversa;
  perguntado sobre o "bicho de estimação", o modelo respondeu refletindo o fato corretamente —
  prova de que a injeção chega no modelo de verdade, não só nos testes automatizados.
- `cargo test -p warden-core -p warden-bootstrap` (99 testes, todos os novos inclusos),
  `cargo check --workspace` e `cargo clippy --workspace --all-targets` limpos.
- Atualizados `PHASE.md` (Fase 4.6 nova), `PENDING.md` (P52 — parte 1 marcada resolvida, parte 2
  segue aberta), `ROADMAP.md`.

**Próximo passo**: parte 2 da P52 (UI de visualização do vault no desktop) segue em aberto, sem
data definida — mesma lista de frentes soltas de antes (Fase 7.6, P8, P46-P51, P58).

---

### 2026-09-08 — Sessão 57 (continuação 2)

- **Objetivo**: usuário disse "bora para a parte 2 da p52 então". Escopo fechado antes de
  planejar (2 perguntas): só leitura (edição continua por fora/pela IA, editor completo fica pra
  v2) e os 3 arquivos fixos destacados numa seção própria, separados da árvore do resto do vault.
  Planejado em modo formal (`/plan`, 1 agente Explore mapeando telas existentes do desktop,
  comandos Tauri e renderização de markdown) antes de codar. P52 fecha por completo nesta sessão
  (as duas partes).

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, seis entradas novas "Visualização
do vault (P52, parte 2)"):

- **Dois comandos Tauri novos** (`desktop/src-tauri/src/vault_cmds.rs`, mesmo precedente de módulo
  dedicado que `sync_cmds.rs` já documentava): `list_vault_files` (reaproveita
  `Vault::list_all_files`, filtra fora os 3 arquivos fixos só quando na raiz) e `read_vault_file`
  (reaproveita `Vault::read` — caminhos só vêm do que a própria lista retornou, sem sanitização
  extra necessária). Ambos reaproveitam o `Vault` já vivo em `AppState.orchestrator`
  (`state.orchestrator.lock().unwrap().clone()`, mesmo padrão de `send_message`), sem reconstruir
  nada do zero.
- **`VaultView.tsx` novo** (mesmo formato de `SyncView`/`UsageView`) — seção "Fixed memory" no topo
  (3 entradas hardcoded, rótulos em português, mesma ordem de `standing_memory`) + árvore simples
  do resto do vault (`buildTree`, função pura, pastas antes de arquivos, alfabética) + painel de
  conteúdo renderizado via `ReactMarkdown`/`remark-gfm` (dependência já existente, usada antes só
  em `MessageBubble.tsx` — `MarkdownLink` virou `export` em vez de duplicado). Arquivo fixo vazio
  mostra placeholder discreto; erro de leitura de um arquivo qualquer da árvore fica isolado ao
  painel, sem derrubar a navegação.
- Entrada nova na sidebar (`VaultIcon`, `Icons.tsx`) entre Sync e Settings; `App.tsx`/
  `Sidebar.tsx` ganharam `"vault"` na união de views.
- `cargo check --workspace`/`cargo clippy --workspace --all-targets` e `npm run build` (tsc+vite)
  limpos.
- **Verificado via Playwright contra o dev server real** (`npm run dev`, mock de
  `list_vault_files`/`read_vault_file`) — seção fixa, árvore agrupada por pasta (`notes/`, `study/`
  antes de `a.md` solto), seleção trocando o conteúdo renderizado (título/itálico/lista/negrito),
  placeholder de arquivo fixo vazio, claro e escuro, todos conferidos por screenshot.
- **Verificado também contra o vault real do usuário** (`npm run tauri dev`, sem mock — ~4:48min
  de build limpo): app abriu e ficou rodando sem crash (confirmado por processo vivo + log sem
  erro), e os 3 arquivos fixos foram seedados de verdade em `~/Warden/vault/` (estava vazio até
  agora) com o conteúdo exato do template — prova de que o bootstrap real resolve o path certo e
  escreve no vault de verdade, não só em teste. Não deu pra capturar screenshot da janela nativa
  (compositor Wayland do ambiente não suporta o protocolo de captura de tela) nem clicar em "Vault"
  sem automação de input (mesma lacuna já aceita em sessões anteriores) — aceito como suficiente,
  já que a UI em si foi inteiramente verificada via Playwright.
- Atualizados `PHASE.md` (Fase 4.6 agora cobre as duas partes), `PENDING.md` (P52 movida pra
  "Resolvidas" — fecha de vez), `ROADMAP.md`.

**Próximo passo**: nenhum item específico escolhido — seguem em aberto Fase 7.6 (build/deploy
mobile), P8 (polish do CLI), o resto do backlog (P46-P51), e P58 (visual do mobile).

---

### 2026-09-08 — Sessão 56

- **Objetivo**: usuário disse "bora continuar?". Escolhido entre as frentes em aberto (4.5 busca
  semântica, 7.5/7.6 mobile push/deploy, P8 polish do CLI): **Fase 4.5 — busca semântica no
  vault** (P6), a única que fecha a Fase 4 inteira. Decisão de arquitetura (embedding local vs
  API) posta ao usuário com trade-offs antes de planejar — escolheu **local via ONNX**. Planejado
  em modo formal (`/plan`, com 2 agentes Explore em paralelo mapeando padrões de crate/config/tool
  e o vault/sync) antes de codar, dado o tamanho e a decisão de arquitetura nova.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, entrada "Fase 4.5"):

- **`Vault::search_semantic` novo** (`crates/warden-core/src/memory/{mod.rs,semantic.rs}`) —
  embedding local via `fastembed` (ONNX, modelo `AllMiniLML6V2`), mesma `SearchHit` que o grep
  original (`Vault::search`, intocado). Índice (`.warden/semantic_index.json`) dentro do próprio
  vault, dot-prefixado — já ignorado por `list_files`/`list_all_files`/sync sem tocar
  `warden-sync`. Self-healing: rehash sha256 por chunk (janela fixa de 40 linhas) a cada chamada,
  reembeda só o que mudou — cobre edição do vault por fora do Warden (Obsidian-compatible), sem
  precisar de hooks nos dois pontos de escrita (`WriteFileTool`, `bundle::apply_bundle` do sync)
- `Orchestrator::handle_turn_streaming` roda a busca semântica dentro de `tokio::task::spawn_blocking`
  (primeiro uso desse padrão no projeto — inferência ONNX é síncrona e pesada) e cai pro grep
  original em qualquer erro (sem rede no primeiro download do modelo, índice corrompido) — resiliente
  sem precisar de um toggle novo em `config.toml`/Settings
- **Verificado de ponta a ponta com o modelo real baixado de verdade** (rede disponível neste
  ambiente, ao contrário da maioria das outras pendências "sem infra externa" do projeto): ranking
  correto distinguindo "consulta médica" de "compromisso com dentista" sem nenhuma palavra em
  comum, e refresh incremental confirmado após editar um arquivo (`crates/warden-core/tests/semantic_search.rs`,
  `#[ignore]`d por padrão pra `cargo test`/CI ficarem hermético e rápido)
- `cargo test -p warden-core` (22 testes), `cargo check --workspace`, `cargo clippy --workspace
  --all-targets` e `cargo test -p warden-sync -p warden-bootstrap -p warden-cli` limpos
- **Achado no meio do caminho, sem relação com a decisão em si**: o ambiente de dev ficou com
  `/home` praticamente cheio (263MB livres) — `target/` (43GB) somado às dependências pesadas do
  `fastembed` (`ort`/`tokenizers`/`image`) estourou o disco, causando um `Bus error` no linker ao
  compilar os testes do `desktop`. `cargo clean` liberou 48GB, mas uma segunda tentativa de
  `cargo test --workspace` completo esvaziou o `target/` sozinho no meio da build (aparenta ser
  alguma limitação de armazenamento do próprio ambiente) — contornado verificando os crates
  individualmente (`check --workspace`/`clippy --workspace --all-targets` cobrem todos os 12,
  `test` escopado nos que mais exercitam `Vault`) em vez do workspace inteiro de uma vez
- Atualizados `PHASE.md` (4.5 concluída, Fase 4 inteira fechada), `PENDING.md` (P6 resolvida, P56
  nova — fallback pro grep nunca exercitado contra uma falha de verdade), `OVERVIEW.md` (status
  geral da Fase 4)

**Próximo passo**: Fase 7.5/7.6 (mobile: push notifications, build/deploy) ou P8 (polish do CLI),
conforme prioridade do usuário na próxima sessão. Vale o usuário confirmar `cargo test --workspace`
numa máquina com mais espaço em disco, se quiser fechar de vez o `#[ignore]` mental sobre isso.

---

### 2026-09-08 — Sessão 56 (continuação)

- **Objetivo**: usuário disse "bora pro 7.5 ent" (Fase 7.5, retomando a lista de opções deixada no
  fim da parte anterior desta sessão). `PHASE.md` só tinha "Notificações push" como título, sem
  nenhum detalhe de escopo — perguntado ao usuário o que deveria disparar a notificação: escolheu
  **notificação local, com o app vivo em background** (`flutter_local_notifications`), não push de
  verdade via FCM/APNs (rejeitando explicitamente a opção de push real depois de um primeiro
  esbarrão sem querer nessa escolha — corrigido pelo usuário antes de eu seguir). Planejado em modo
  formal (`/plan`, 1 agente Explore mapeando a arquitetura do app Flutter) antes de codar.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, entrada "Fase 7.5"):

- **`mobile/lib/services/chat_notifications.dart` novo** — `shouldNotifyFor`/`notificationContentFor`
  puras e testadas (`mobile/test/services/chat_notifications_test.dart`, 5 testes novos, mesmo
  padrão pure-vs-plugin do `warden-cli`); `initializeChatNotifications`/`requestNotificationPermission`/
  `showChatNotification` finas sobre `flutter_local_notifications`. Gatilho em `_ChatScreenState`
  via `WidgetsBindingObserver`/`AppLifecycleState` — como não existe hoje nenhum jeito de navegar
  pra fora do chat sem desconectar (P41), `state != resumed` já resolve "devo notificar?" sozinho
- **Achado real, sem relação com a decisão em si**: `fastembed` (Fase 4.5 da sessão anterior)
  quebrava a compilação cruzada do `warden-mobile-bridge` pra Android — `ort` sem binário
  pré-compilado pra `armv7-linux-androideabi`, e os defaults do `fastembed` puxando
  `native-tls`/`openssl-sys` (que também não cross-compila). Nunca tinha aparecido porque a 4.5 só
  foi testada em host x86_64 — essa foi a primeira tentativa de build Android desde então.
  Corrigido: `semantic-search` virou uma feature opcional em `warden-core` (default-on), desligada
  só em `warden-sync` (nunca usa `Orchestrator`, só I/O de arquivo — e é por onde
  `warden-mobile-bridge` alcança `warden-core`), e `fastembed` trocado pra rustls
- **Segundo achado real**: `main.dart` faltava `WidgetsFlutterBinding.ensureInitialized()` antes de
  `initializeChatNotifications()` — inofensivo enquanto só `RustLib.init()` (FFI puro) rodava antes
  do `runApp`, mas `flutter_local_notifications` fala por `MethodChannel`, que exige o binary
  messenger pronto; sem isso o app crashava direto na abertura (tela em branco)
- **Verificado de ponta a ponta contra hardware real (emulador `warden_test`), zero mock, nenhum
  passo pulado**: subiu um `warden-server` real de teste (chave OpenAI inválida de propósito, pra
  ter uma resposta rápida e determinística sem gastar cota de API real), app conectado via
  `10.0.2.2`, prompt de permissão de notificação real aceito e confirmado via `adb shell dumpsys
  package`, mensagem mandada, app levado pro background antes da resposta chegar, e a notificação
  real do Android apareceu na bandeja com o conteúdo certo (confirmado por `dumpsys notification` —
  `NotificationRecord` de verdade, canal `chat_messages` — e uma screenshot da bandeja puxada)
  enquanto o processo seguia **não congelado** (`dumpsys activity processes` → `isFrozen=false` o
  tempo todo, descartando a hipótese de que o Android estava só "congelando" o app). Toque na
  notificação reabriu a `ChatScreen` com a conversa intacta. **Lição de metodologia**: as primeiras
  tentativas de automatizar o envio via `adb shell input tap` erraram a posição do botão de enviar
  porque a barra de input muda de lugar na tela dependendo do teclado estar aberto ou não —
  resolvido lendo `uiautomator dump` pra pegar as coordenadas reais em vez de estimar pela
  screenshot
- `flutter analyze`/`flutter test` (38 testes) e `cargo check --workspace`/`cargo clippy --workspace
  --all-targets`/`cargo test -p warden-core` limpos
- Atualizados `PHASE.md` (7.5 concluída, 7.6 registrada como não pedida/fora de escopo),
  `PENDING.md` (P57 nova — notificações são Android-only, sem equivalente iOS), `OVERVIEW.md`
  (status da Fase 7 quase completa)

**Próximo passo**: Fase 7.6 (build/deploy de release) se o usuário quiser publicar de verdade em
algum momento, ou P8 (polish do CLI) — nenhum dos dois pedido ainda.

**Depois de entregue a 7.5**, usuário comentou que ainda tem muita coisa pra mexer, quer testar o
app no celular real dele antes de qualquer trabalho visual, e quer continuar os pontos já
registrados no backlog (P45-P52 etc.) — mas sem escolher qual agora. Pedido explícito: só anotar,
sem implementar nada disso ainda. Registrado como **P58** em `PENDING.md` (identidade visual do
mobile não alinhada com a marca — achado concreto: `Colors.deepPurple` genérico do Material em vez
dos tokens reais `#7c3aed`/`#a78bfa` do desktop) — escopo exato (só cor vs. revisão visual mais
ampla) fica pra quando o usuário tiver testado no celular dele e voltar com o que incomodou de
verdade.

---

### 2026-09-07 — Sessão 55

- **Objetivo**: usuário disse "bora continuar?". Escolhido entre as frentes em aberto (Mobile 7.5/7.6,
  Vault mobile + busca semântica 4.4/4.5, ou polish do CLI P8): **Fase 4.4 — vault local + sync
  pleno no app mobile**, e dentro dela, 4.4 antes de 4.5 (maior risco de toolchain, já que era a
  primeira ponte Rust↔Flutter do projeto — melhor validar logo). Planejado em modo formal (`/plan`)
  antes de codar, dado o tamanho e o risco novo de toolchain.

**O que foi feito** (detalhes completos em `ARCHITECTURE.md`, entrada "Fase 4.4"):

- **Novo crate `crates/warden-mobile-bridge`** — casca fina sobre o mesmo `warden_sync::SyncEngine`
  que o desktop/CLI já usam (Sessão 54), exposta via `flutter_rust_bridge` (FRB) com toolchain de
  build **cargokit** (não Gradle/Xcode escritos à mão). Funções espelhando os 7 comandos Tauri de
  `sync_cmds.rs`, recebendo os 4 paths do sync (vault/config/secrets/manifest) como `String`
  explícitos do Dart — mobile não tem `dirs::config_dir()` confiável
- Métodos async do `SyncEngine` (Tokio por baixo) rodam via um `tokio::runtime::Runtime` próprio
  (`OnceLock`) dentro do bridge, `.block_on()`'d de dentro de funções `pub fn` simples que o FRB já
  despacha pra thread de fundo por padrão — nunca trava a UI do Dart
- `mobile/lib/services/vault_paths.dart` — paths resolvidos via `path_provider`'s
  `getApplicationSupportDirectory()`. Nova `mobile/lib/screens/sync_screen.dart` (status, init,
  send/pull com QR via `qr_flutter`, pareamento host/join), alcançável por um ícone novo na AppBar
  da `ConnectionScreen`, independente de estar conectado ao `warden-server`
- **Campo de "host override" no pareamento** (não só sweep de LAN automático) — decisão de
  produto: `pairing_join_with_hosts` já existia no `SyncEngine` (só usado em teste), exposto na UI
  porque o sweep automático não atravessa NAT de emulador nem wifi com isolamento de cliente
- Toolchain instalado: `cargo-ndk`, `flutter_rust_bridge_codegen` — o NDK Android em si
  (`~/.local/opt/android-sdk/ndk` 27/28) já existia de sessões anteriores, só reaproveitado
- **Verificado de ponta a ponta contra hardware real (emulador `warden_test`), sem mock**: `.so`
  compilado pras 4 ABIs Android via `cargo-ndk`; APK instalado/aberto via `adb`; `bridge_status()`
  chamado de verdade on-device; **pareamento real** contra um segundo processo `SyncEngine`
  isolado (path de teste dedicado — nunca tocou o `~/.config/warden/` real do usuário) usando o
  override `10.0.2.2`, confirmado via `adb run-as` que o `vault_key` ficou **idêntico** nos dois
  lados (prova a troca ECIES completa); e o motor de diff/hash rodando de verdade dentro do `.so`
  — um arquivo escrito no vault local do emulador fez `bridge_status` reportar
  `pending_vault_changes: 1`. `cargo build/test/clippy --workspace` e `flutter analyze`/
  `flutter test` (28 testes) limpos
- **Fora desta fatia, mesmas lacunas já aceitas em outras pendências**: push/pull reais contra
  Arweave/TruthID (extensão de P38/P55), lado iOS (scaffold no lugar, nunca buildado — sem
  Xcode/macOS, mesma lacuna de P39/P44), UI de navegação/edição do conteúdo do vault (nem
  desktop/CLI têm isso hoje)
- Atualizados `PHASE.md` (4.4 concluída), `PENDING.md` (P53 resolvida), `OVERVIEW.md` (status geral
  + correção de uma linha desatualizada "Mobile: Tauri" pra "Flutter")

**Próximo passo**: Fase 4.5 (busca semântica no vault) — independente da 4.4, sem bloqueio de
toolchain; ou retomar Fase 7.5/7.6 (mobile: push notifications, build/deploy) ou P8 (CLI), conforme
prioridade do usuário na próxima sessão.

---

### 2026-09-06 — Sessão 54

- **Objetivo**: usuário disse "bora continuar?". Depois de fechar a leva de ideias da Sessão 53
  (registro sem implementação), retomado o **P37** — sync descentralizado do vault + `config.toml`
  via Arweave com o TruthID pagando (`pin()`). P37 tinha ficado com três decisões em aberto desde a
  Sessão 50 (formato do manifesto, descoberta da "última versão" sem tags no Arweave, identidade
  que deriva a chave de cifra) — esta sessão fechou as três e implementou o motor completo.
  Planejado em modo formal (`/plan`) antes de codar.

**Decisões-chave** (detalhes completos em `ARCHITECTURE.md`, entrada "warden-sync (Sessão 54)"):

- **Novo crate `crates/warden-sync`**, consumido pelo desktop (`desktop/src-tauri`) e pelo
  `warden-cli` (não pelo `warden-bootstrap`, que é `deny_unknown_fields`). `SyncEngine` é a única
  superfície publica: `status`/`init_fresh`/`begin_push`/`finish_push`/`pull`/`pairing_host`/
  `pairing_join`. Estado em dois JSON novos em `~/.config/warden/` (separados do `config.toml`):
  `sync_secrets.json` (`device_id` + `vault_key` de 32 bytes, **nunca** reescrito depois de criado)
  e `sync_manifest.json` (hash sha256 por arquivo do vault + hash do `config.toml` +
  `owner_address`/`last_tx_id`/`manifest_counter`, reescrito a cada push/pull)
- **Formato do manifesto (decisão 1)**: diff tipo-git, hash sha256 por arquivo, via `Vault::list_all_files()`
  novo em `warden-core` (todos os arquivos, não só `.md` — `list_files` original ficou intocado).
  Bundle de um push é um envelope JSON (arquivos mudados em base64 + deletados + config se mudou)
  cifrado como **um blob só** reaproveitando `warden_truthid::crypto::encrypt_pin_content`/
  `decrypt_pin_content` — resolve também a restrição de sem-batching do `pin()` (uma aprovação
  física por chamada → N arquivos viram um blob por sync)
- **Descoberta da "última versão" (decisão 2)**: como `pin()` não permite tags customizadas,
  consultado o GraphQL do Arweave (`transactions(owners:[...], sort:HEIGHT_DESC)`) pelo endereço
  da carteira do TruthID — aprendido uma vez (via `transaction(id:...){owner{address}}` sobre a tx
  do primeiro push) e propagado a outros devices pelo pareamento, sem ponteiro copiado à mão
- **Identidade da chave (decisão 3)**: segredo próprio do Warden (`SyncSecrets.vault_key`), nunca
  visto pelo TruthID/Arweave, gerado no primeiro device e espalhado pelos demais via **protocolo de
  pareamento novo** — código curto (8 chars, alfabeto sem `0/O/1/I/L`) + LAN, sem câmera/QR
  (decisão explícita do usuário: pareamento agora é só entre dispositivos com vault — desktop↔desktop
  ou desktop↔`warden-cli` — não mais desktop↔celular, então QR perdeu a vantagem da câmera).
  Reaproveita só os primitivos genéricos de `warden-truthid` (ECIES, `lan::candidate_hosts()`), com
  faixa de portas própria (`48070-48074`, distinta de `LAN_PORTS` do TruthID) e um listener de
  verdade (`tokio_tungstenite`/`accept_async`, não `axum` — que no projeto só existe em testes).
  Quem mostra o código é o host; o joiner varre a LAN, prova o conhecimento do código via
  `code_proof` (HMAC derivado do código, o código em si nunca trafega) e recebe a chave via ECIES
- **Bug real achado pelo teste de ponta a ponta**: a primeira versão do pareamento também propagava
  `last_tx_id` do host pro dispositivo que entra — isso fazia o primeiro `pull` do novo dispositivo
  achar que "já estava atualizado" sem nunca ter baixado nada (manifest `last_tx_id` batia com o tx
  mais recente sem `vault_files` correspondente). Corrigido removendo `last_tx_id` do payload de
  pareamento — só a chave e o `owner_address` viajam; a versão real só vem de um `pull` de verdade
- **Política de conflito do v1**: last-write-wins, sem merge de 3 vias — `pull` compara hashes
  contra o manifesto **antigo** antes de sobrescrever pra avisar sobre mudança local perdida
  (warning na UI), mas não bloqueia

**Implementado** (ver `ARCHITECTURE.md` pros detalhes completos):

- `crates/warden-sync`: `manifest.rs`, `diff.rs`, `bundle.rs` (envelope cifrado), `arweave.rs`
  (cliente GraphQL simples via `reqwest`, tres queries: `latest_tx_by_owner`/`owner_of_tx`/
  `fetch_tx_data`), `push.rs` (`begin_push`/`finish_push` separados, mostra o QR antes de bloquear
  no telefone), `pull.rs`, `pairing/` (`host.rs`/`join.rs`/`protocol.rs`)
- **Integração desktop**: `desktop/src-tauri/src/sync_cmds.rs` novo (7 comandos Tauri mostrados no
  invoke handler: `sync_status`/`sync_init`/`sync_push_begin`/`sync_push_await`/`sync_pull`/
  `pairing_start`/`pairing_join` — mostrar o QR e bloquear no telefone são IPC separados, e o
  pareamento roda em background emitindo `pairing-completed`/`pairing-failed`), QR renderizado como
  **SVG inline** via crate `qrcode`, tela `SyncView.tsx` nova (status, botões Enviar/Pull, fluxo de
  pareamento mostrar/digitar código), item "Sync" na sidebar (`SyncIcon` novo em `Icons.tsx`)
- **Integração CLI**: `/sync` (status local, sem tocar na rede), `/sync push` (QR em **Unicode
  direto no terminal** via `qrcode::render::unicode::Dense1x2` — funciona por SSH numa máquina sem
  tela), `/sync pull`, `/sync pair` (mostra código e bloqueia aguardando join), `/sync pair <code>`
  (digitar código). `--vault-path` do CLI passado adiante pro `/sync` resolver o mesmo vault que a
  sessão usa (`resolve_vault_path` espelha a precedência do próprio `bootstrap`)
- **34 testes** (27 unitários em `warden-sync` + `tests/engine_lifecycle.rs` + `tests/fake_arweave_gateway.rs`
  + `tests/fake_pairing_peer.rs` + `tests/fake_phone_push.rs` — round-trip completo simulando dois
  devices via telefone/gateway/par de pareamento falsos, mesmo padrão do `fake_phone.rs` do
  `warden-truthid`). `cargo build/test/clippy --workspace` e `tsc` limpos
- `project/PHASE.md` (4.1 motor / 4.2 integração desktop / 4.3 integração CLI marcadas `[x]`,
  lista antiga de etapas IPFS substituída), `project/OVERVIEW.md` (IPFS → Arweave, status da Fase 4),
  `project/ARCHITECTURE.md` (decisão do crate registrada em detalhe), `project/PENDING.md` (P37
  movida pra Resolvidas; P53/P54/P55 novos)

**Ainda falta**: P53 (mobile sem vault local — exigiria `flutter_rust_bridge`, primeira ponte
Rust↔Flutter, deliberadamente fora desta fatia), P54 (credenciais OAuth de MCP fora do sync),
P55 (nunca testado contra o TruthID/Arweave reais — só fakes, maior risco é a query GraphQL exata
bater com o schema real do gateway `arweave.net`). 4.4 (mobile) e 4.5 (busca semântica) seguem
abertas no `PHASE.md`.

**Próximo passo**: perguntar ao usuário se segue pra 4.4 (mobile, custo de toolchain alto) ou volta
pros demais candidatos da Sessão 52/53 (7.5/7.6, Fase 9 9.3/9.4, ou uma das 8 ideias da Sessão 53).

---

### 2026-09-06 — Sessão 53

- **Objetivo**: usuário trouxe uma leva grande de ideias novas pro projeto, dadas de uma vez e
  cru ("são apenas ideias pro projeto"), sem pedir implementação nenhuma. Nenhum código
  mudado nesta sessão — só registro em `PENDING.md` (P45-P52) e `ROADMAP.md` (seção
  "Ideias de Expansão"), como o protocolo do projeto pede pra ideia nova/pendência sem `/plan`.

**Ideias registradas** (detalhes completos em `ROADMAP.md`/`PENDING.md`):

- **P45** — um agente por conversa, escolhido no início (trava depois), evolução do seletor
  atual (Sessão 43)
- **P46** — orquestração de agentes em dois modos possíveis: um "chefe" central que cria/comanda
  outros agentes, ou vários agentes "funcionários" independentes sem nenhum global — evolui
  "Sub-agentes autônomos"/P8
- **P47** — SSH pra VPS/máquinas externas: cadastro de chaves nas configurações, permissão
  concedida/negada à IA por chave
- **P48** — avatares/personas 3D pros agentes (geração via prompt e/ou foto, animações, avatar
  que se move na tela)
- **P49** — overlay "Super Jarvis": atalho global de teclado abre busca/voz sem abrir o app,
  avatar aparece ali — evolução do "Copilot" (P9)
- **P50** — tier pago: hospedar o servidor (Fase 9) pelo próprio Warden, sem o usuário precisar
  de VPS/casa própria — primeira menção de modelo de negócio pago no projeto
- **P51** — "9Router": nome concreto pra evolução da "Warden API" (P12) — API do agente pessoal
  (vault + personalidade) pra usar em outros harnesses, com OAuth de contas de provedores de IA
  (Claude, GPT, etc.); usuário sinalizou que é hora de puxar isso pra frente
- **P52** — estrutura padrão do vault (parte fixa: perfil do usuário/comportamento da IA; parte
  livre: o resto, a critério da IA) + visualização fácil do vault pela interface, não só
  markdown cru — pra Fase 4, relacionado a P37 (sync via Arweave)

**Próximo passo**: nenhuma decisão de prioridade tomada ainda sobre essas 8 ideias — quando o
usuário quiser avançar alguma, vale revisitar a ordem do `ROADMAP.md` (topo do arquivo) e
decidir onde encaixar.

---

### 2026-09-06 — Sessão 52

- **Objetivo**: usuário disse "commita e da push" (feito, commit `b388051`), depois "por onde
  continuamos?". Candidatos: 7.4 (tools locais no mobile), P37 (design do sync Arweave), Fase 9
  (9.3/9.4). Usuário escolheu **7.4**.

**Decisão de escopo, antes de planejar**: como o modelo roda no `Orchestrator` que o
`warden-server` hospeda (7.3), não no celular, uma tool "local" só funciona com um mecanismo de
roteamento de tool pra conexão certa — exatamente o que `PHASE.md` reservava pra 9.4/9.5.
Perguntado ao usuário qual tool faria sentido pra começar (já que "shell, arquivos" do `PHASE.md`
original não cabe num celular sem root) — escolheu **acesso a arquivos**. Perguntas de escopo
adicionais antes de planejar: pasta raiz persistida (vs. picker por chamada) — escolhida a
persistida; só leitura (vs. leitura+escrita) — escolhida só leitura. Planejado em modo formal
(`/plan`).

**Implementado** (ver `ARCHITECTURE.md`, seção "7.4" pros detalhes completos):

- Mecanismo genérico de roteamento de tool em `crates/warden-server`: `Hello` ganha
  `tools: Vec<ToolSpec>` (retrocompatível), novo par `ToolCallRequest`/`ToolCallResult`+
  `ToolCallError`, novo `RemoteTool` (`src/remote_tool.rs`) — um `Tool` que manda a chamada pela
  conexão e espera a resposta via `oneshot`, compartilhando um mapa `pending` por conexão. Uma
  conexão que anuncia tools ganha seu próprio `Orchestrator` (clone barato do compartilhado) com um
  `RemoteTool` por spec — a primeira peça concreta do que 9.4/9.5 vai generalizar depois
- **Bug real achado rodando o teste de ponta a ponta**: as tools do celular nomeadas
  `list_files`/`read_file` colidiam com `ReadFileTool`/`WriteFileTool` do vault (mesmos nomes) —
  Gemini rejeitou com 400 "Duplicate function declaration found". Renomeado pra
  `list_phone_files`/`read_phone_file`. Registrado como P42 (sem validação de colisão no servidor
  ainda)
- **Achado de pesquisa antes de escrever código**: a escolha óbvia de pacote Flutter
  (`shared_storage`) está descontinuada, sem sucessor listado — rastreado até `saf_util`+
  `saf_stream` (mesmo autor, ativamente mantidos), confirmado lendo o código-fonte instalado, não
  só a doc do pub.dev
- `mobile/lib/services/mobile_file_tool.dart` novo (pick de pasta via SAF, `list_phone_files`/
  `read_phone_file`, design de "path opaco" — o modelo só ecoa paths que já viu, nunca constrói
  um), `ConnectionScreen`/`ChatScreen` ganham a UI (diálogo "Files" no `AppBar`, opt-in — só
  anuncia as tools se uma pasta já foi escolhida)
- Testes: `crates/warden-server/tests/tools.rs` (round-trip completo, resposta final derivada do
  conteúdo REAL que o cliente de teste devolveu — não uma string enlatada), `remote_tool.rs` ganha
  testes unitários (sucesso, erro, timeout, conexão caída). Lado Flutter: `flutter analyze`/
  `flutter test` cobrindo `Hello.tools`/`ToolCallRequest`/`Result`/`Error`. `cargo build/test/
  clippy --workspace` e `flutter analyze`/`flutter test` limpos nos dois lados
- **Verificado de ponta a ponta contra um `warden-server` real com Gemini de verdade**: dois
  arquivos reais empurrados pro emulador via `adb push`, pasta escolhida via o picker real do
  Android (SAF, diálogo de permissão real aceito), pergunta real respondida corretamente citando
  os arquivos reais e o conteúdo real de um deles — a cadeia inteira (Gemini → servidor →
  `RemoteTool` → rede → SAF real no Android → de volta → resposta) funcionando de verdade
- `project/PHASE.md` (7.4 marcada `[x]`), `project/OVERVIEW.md` (status da Fase 7 atualizado),
  `project/PENDING.md` (P42 novo — colisão de nome não validada; P43 novo — escrita deliberadamente
  fora de escopo; P44 novo — Android-only, sem iOS)

**Ainda falta**: 7.5 (push), 7.6 (build/deploy). P40/P41 (Sessão 51, ainda abertas). P42/P43/P44
(acima). Sem Tailscale real disponível neste ambiente (P36, mesma lacuna de sempre).

**Próximo passo**: perguntar ao usuário se segue pra 7.5/7.6, volta pro design do sync Arweave
(P37), ou outra frente da Fase 9 (9.3/9.4).

---

### 2026-09-06 — Sessão 51

- **Objetivo**: usuário disse "bora continuar?". Recapitulado o estado (7.1/7.2 concluídas na
  Sessão 50, P37/Arweave em design, Fase 9 9.3/9.4 em aberto). Perguntado por onde seguir —
  usuário escolheu **7.3, interface de chat mobile**.

**Decisão de escopo, antes de codar**: constatado que o protocolo `warden-server` só tinha
`Hello/Ping/Goodbye` — não dava pra ter chat de verdade sem o servidor rodar um `Orchestrator`
real (reabrindo de propósito a decisão da 9.2 de não ter um). Perguntado ao usuário: chat real
(servidor hospeda `Orchestrator`) vs. UI de chat só de mentirinha sem back-end — escolheu **chat
real**. Planejado em modo formal (`/plan`) antes de codar.

**Implementado** (ver `ARCHITECTURE.md`, seção "7.3 — Chat real no Flutter" pros detalhes
completos):

- `crates/warden-server`: protocolo ganha `Chat`/`ChatResponse`/`ChatError`; `Server` passa a
  hospedar um `Arc<Orchestrator>` (via `bootstrap()`, chamado uma vez no `main.rs`, mesmas flags
  do `warden-telegram`) e responde `Chat` com o modelo de verdade, uma conversa por `device_id`
  (`warden_bootstrap::handle_turn`, nova `default_server_conversations_dir()`)
- **Bug de concorrência achado e corrigido por raciocínio, antes de qualquer teste**: tratar
  `Chat` inline no loop de leitura da conexão travaria heartbeats do cliente por até 70s+ (latência
  real do Gemini já registrada em sessões anteriores), derrubando conexões saudáveis. Corrigido com
  um canal `mpsc` + task de escrita dedicada — o loop de leitura nunca mais bloqueia num `Chat` em
  andamento
- Testes novos no `warden-server` (`MockProvider` sem chave de API real, reaproveitando
  `warden_core::model::response_stream`): round-trip de chat, caminho de erro, e um teste que
  prova o fix de concorrência (`Ping` durante um `Chat` de 2s de delay artificial chega em menos de
  500ms). `cargo build/test/clippy --workspace` limpos (7 testes novos)
- Flutter: `ChatMessage`/`ChatResponseMessage`/`ChatErrorMessage` no protocolo,
  `sendChat`/`chatStream` no `ServerConnection`, novo `ChatScreen` (bolhas de mensagem, indicador
  de "Thinking…", um turno por vez, banner de status se a conexão cair). `ConnectionScreen` navega
  pra lá após conectar, sem forçar desconexão ao voltar. `flutter analyze`/`flutter test` limpos
  (20 testes, 6 novos)
- **Verificado de ponta a ponta contra um `warden-server` real com Gemini de verdade** (config
  real do usuário, não mockado): AVD `warden_test`, duas mensagens reais respondidas e renderizadas
  corretamente; a segunda (pedido de poema) demorou o bastante pra passar de um ciclo de heartbeat
  de 30s — confirmado por screenshot e pelo log do servidor que a conexão não caiu, provando o fix
  de concorrência também no caminho real
- `project/PHASE.md` (7.3 marcada `[x]`), `project/OVERVIEW.md` (status da Fase 7 atualizado),
  `project/PENDING.md` (P40 novo — sem histórico ao reconectar; P41 novo — sem caminho de volta pra
  `ChatScreen` sem desconectar, achado durante a verificação manual; P36 atualizado — a lacuna de
  "sem tailnet real" cobre agora tráfego de chat também)

**Bônus fora do escopo original, pedido pelo usuário no meio do trabalho**: usuário reportou "a
barra lateral esquerda de menu não recolhe" — confirmado que nunca existiu mecanismo de collapse
na sidebar do desktop (não é regressão, feature nunca construída). Perguntado como deveria
recolher — usuário escolheu **rail de ícones** (logo + nova conversa + Usage + Settings só com
ícone, lista de conversas some) sobre esconder de vez. Implementado: `sidebarCollapsed` em
`App.tsx` persistido via `localStorage`, `Sidebar.tsx`/`ChevronIcon` novos, CSS estreitando a
coluna do grid de 280px pra 64px. Verificado com Playwright headless contra o `vite dev` real
(claro e escuro, expandir/recolher/expandir de novo), zero erro de console. `tsc`/`npm run build`
limpos.

**Ainda falta**: 7.4 (tools locais no mobile), 7.5 (push), 7.6 (build/deploy). P40/P41 (acima).
Sem Tailscale real disponível neste ambiente (P36, mesma lacuna de sempre).

**Próximo passo**: perguntar ao usuário se segue pra 7.4 (tools locais), volta pro design do sync
Arweave (P37), ou outra frente da Fase 9.

---

### 2026-09-06 — Sessão 50

- **Objetivo**: usuário disse "bora continuar?". Resumido o estado (Sessão 49 fechou a 7.1 Android;
  próximo passo natural era 7.3 — layout responsivo mobile — ou P1, que bloqueava a 7.2).
  Perguntado ao usuário, escolheu **fechar o P1 primeiro** (protocolo servidor↔cliente).

**O que foi feito**:

- Antes de decidir, conferido o SDK real do TruthID (`~/Documents/workspace/truthid/docs/docs/sdk/dart.md`,
  `TruthIDRequester`) pra validar a premissa registrada em `PENDING.md` P1 ("TruthID já tem um relay
  stateless por WS — reaproveitar seria natural") — **premissa incorreta**: o mecanismo real de lá
  não usa relay nenhum, é local-network sweep + IPFS/IPNS dead-drop ("No relayer, no TruthID server,
  no polling endpoint for you to host"), resolvendo pareamento entre dois devices sem VPN
  compartilhada — um problema que o Warden não tem, já que assume Tailscale (`CONTEXT.md`,
  `OVERVIEW.md`) como malha de conectividade. Nada do TruthID foi reaproveitado na decisão final
- Apresentada ao usuário a análise sem essa suposição: WebSocket vs gRPC como protocolo de
  *aplicação* rodando dentro do túnel já criptografado do Tailscale (não mais "qual mecanismo de
  conectividade" — isso já é o Tailscale). Recomendado **WebSocket + protocolo JSON próprio**,
  motivado pela Fase 9 (9.4/9.5: servidor precisa empurrar "execute esta tool" pro cliente certo e
  receber o resultado pela mesma conexão — duplex nativo em WS) e por consistência com os outros
  dois protocolos JSON internos que o projeto já tem (sidecar do WhatsApp, MCP) em vez de introduzir
  protobuf/`tonic`/codegen só pra tipagem forte. Usuário confirmou a recomendação
- `PENDING.md`: P1 movido de "Não Resolvidas" pra "Resolvidas", com a correção da suposição do
  TruthID documentada. `ARCHITECTURE.md`: linha "Protocolo servidor↔cliente" atualizada de "Em
  aberto" pra **WebSocket + protocolo JSON próprio** ✓, com o raciocínio completo e o que ainda fica
  em aberto (formato exato das mensagens por tipo de evento, quem assume o papel de servidor,
  autenticação da conexão WS — tudo isso fica pra quando 7.2/9.2 forem implementadas de verdade,
  fora do escopo desta decisão de protocolo)
- Nenhuma etapa de `PHASE.md` marcada como concluída — P1 era só a decisão de arquitetura que
  bloqueava a 7.2/9.2, não a implementação em si; ambas seguem `[ ]`

**Próximo passo**: com P1 fechado, a 7.2 (conectar o mobile ao servidor via Tailscale/WS) e a 9.2
(implementar o protocolo em si) estão desbloqueadas — mas o usuário ainda não escolheu qual atacar
agora, nem se prefere ir primeiro pela 7.3 (layout responsivo mobile, que não dependia de nada e
segue disponível). Perguntar ao usuário por onde seguir.

**Continuação (ainda 2026-09-06, mesma Sessão 50) — 9.2, implementação**: perguntado por onde
seguir entre 7.3/7.2/9.2, usuário escolheu **9.2** (protocolo em si), já que 7.2 e 9.2 reaproveitam
o mesmo protocolo — implementar a 9.2 primeiro evita desenhar o wire format duas vezes. Planejado
com 2 agentes de exploração em paralelo (arquitetura de `Tool`/`Orchestrator`/`DelegateTool` em
`warden-core`; config/bootstrap e o binário `warden-mcp-server` como precedente de "expor Warden
sobre um protocolo") + 1 agente de design, plano revisado (lendo `Cargo.toml` de crates existentes
pra confirmar convenções) e aprovado pelo usuário antes de codar.

**Implementado**:

- **Novo crate `crates/warden-server`** (bin + lib, diferente do `warden-mcp-server` que é só bin
  — o lado *client* já nasce reutilizável pra Fase 7.2 sem redesenhar o protocolo depois):
  - `src/protocol.rs`: `ClientMessage`/`ServerMessage`, um enum por direção (mesmo estilo do IPC
    do sidecar do WhatsApp, não JSON-RPC), `Hello{device_id,device_name,auth_key}`/
    `Ping{nonce}`/`Goodbye{reason}` e `HelloAck{server_name}`/`AuthError{reason}`/`Pong{nonce}`/
    `Goodbye{reason}`. **Achado real rodando o teste de round-trip JSON**: `rename_all =
    "camelCase"` num enum com `tag = "type"` só renomeia o nome da variante/tag — os campos
    dentro de cada variante continuam snake_case a menos que se acrescente
    `rename_all_fields = "camelCase"` à parte (corrigido, os dois atributos juntos)
  - `src/server.rs`: `Server::bind`/`local_addr`/`serve` (accept loop, uma task por conexão,
    mesmo estilo do `ChildSidecar` do WhatsApp) + `handle_connection` (primeiro frame tem que ser
    `Hello`; chave errada → `AuthError` tipado + WS `Close` código 1008, não um close silencioso;
    chave certa → `HelloAck`; depois só responde `Ping`/loga `Goodbye`/erro de parse)
  - `src/client.rs`: `ServerConnection::connect`/`send`/`recv`/`ping` — a API que a 7.2
    (desktop/mobile como cliente) vai importar direto depois
  - `src/main.rs`: `warden-server` binário, clap (`--listen`, default `0.0.0.0:7420` — porta
    escolhida só pra não colidir com o dev server do Tauri em `1420`; `--auth-key`), resolve a
    chave (`WARDEN_SERVER_AUTH_KEY` env vence sobre a flag, mesmo *padrão* de `resolve_secret` sem
    puxar `warden-bootstrap` por um campo só)
  - `tests/handshake.rs`: 3 testes reais (não mockados) — handshake + heartbeat com múltiplos
    ping/pong, chave errada rejeitada com `AuthError` e conexão fechada
- **Dependência escolhida**: `tokio-tungstenite` (não promover `axum`, que já existe só como
  dev-dependency em `warden-core`) — o `ws` extractor do `axum` traria a pilha HTTP inteira
  (tower/hyper/matchit) pra um servidor com um endpoint só e zero semântica HTTP, mesmo raciocínio
  já usado pro catcher de redirect OAuth. `crates/warden-server` **não depende de
  `warden-bootstrap`/`warden-core`** — zero superfície de tool dispatch nesta peça (isso é 9.4),
  um `Orchestrator` aqui seria peso morto agora
- `cargo build/test/clippy --workspace` limpos (5 testes novos no `warden-server`: 2 de
  round-trip JSON + 3 de integração real client+server), `cargo run -p warden-server` testado
  manualmente de verdade (bindou, logou, encerrado limpo)
- `project/PHASE.md` (9.2 marcada como concluída), `project/ARCHITECTURE.md` (decisão de
  crate/dependência/schema registrada), `project/PENDING.md` (P36 novo — sem TLS próprio, depende
  da 9.1 pra criptografia; chave de auth sem rotação/UI ainda; sem teste sobre tailnet real)

**Ainda falta**: sem Tailscale real pra testar sobre a malha de verdade (mesma lacuna já aceita em
outras partes do projeto por falta de infra externa no ambiente de dev). 9.1 (Tailscale), 9.3
(registro/pareamento), 9.4/9.5 (roteamento de tool pro cliente certo), 9.6/9.7 (workspace de
máquinas, QR pairing) seguem todos em aberto, deliberadamente fora desta sessão.

**Próximo passo**: usuário ainda não escolheu — candidatos são 7.2 (agora desbloqueada, desktop ou
mobile conectando de verdade em um `warden-server` via o `ServerConnection` novo), 7.3 (layout
responsivo mobile, seguia disponível o tempo todo), ou continuar a própria Fase 9 (9.3/9.4).

**Continuação (ainda 2026-09-06, mesma Sessão 50) — direção nova, TruthID+Arweave**: perguntado
"nada que dependa de hardware/emulador agora" como restrição pro próximo passo, usuário mudou de
direção por completo: quer reduzir ao máximo a dependência de servidor sincronizando vault +
`config.toml` entre devices via **Arweave**, usando a carteira do **TruthID** (projeto irmão) como
pagador/publicador em vez do Warden ter carteira própria — "eu quero que o truthid seja o local
onde possa ser cobrado essas taxas". Confirmado logo em seguida, sem meio-termo: **"mas vamos
criptografar tudo certo?"** — tudo cifrado antes de sair do device, não só chaves de API, o vault
inteiro também, decisão não-negociável.

**Investigação em duas rodadas** (agentes em paralelo, código real do TruthID — não só docs — e o
`Vault`/config do Warden):
- Confirmado: a carteira Arweave do TruthID (RSA-4096 JWK) é independente do lado EVM/smart-account
  — dá pra usar como pagador sem herdar nenhuma infra EVM/bundler
- O SDK Dart já expõe exatamente o mecanismo pra apps terceiros: `TruthIDRequester.pin()` — QR, o
  app TruthID escaneia/aprova, cifra em trânsito/decifra/publica no Arweave com a própria carteira,
  devolve `PinResult{cid: "ar://<txid>", ...}`
- **Limitações reais que moldam o design**: sem tags customizáveis no Arweave pro `pin()` de
  terceiro (só `App-Name: TruthID` fixo — inviabiliza descoberta "latest by tag" via GraphQL);
  sem batching (uma aprovação física por chamada — sessão de uso único, LAN + IPFS/IPNS dead-drop
  só pro resultado, nunca pro conteúdo)
- Sem spec do protocolo fora do código Dart — replicar em Rust exige ler o código-fonte real
  byte a byte, não tem documento pra seguir
- Lado Warden confirmado limpo pra receber isso: `Vault` sem hashing/cifra hoje, config isolado das
  conversas (`FileConfig` nunca referencia `Conversation`), nenhuma dependência de hash/cripto no
  workspace ainda

**Perguntas fechadas com o usuário** antes de planejar: ponteiro de "última versão" — usuário pediu
pra usar a carteira do TruthID mesmo (fica como direção, resolvendo o "quem paga" mas deixando o
"como descobrir a versão mais recente" pra uma sessão futura de design do manifesto, já que
`pin()` não dá tags customizáveis); `mcp_servers` **incluído** no escopo cifrado (mesmo nível de
sensibilidade das chaves de API); usuário confirmou entrar em modo de planejamento formal.

**Escopo desta sessão, deliberadamente restrito**: só o cliente Rust do protocolo `pin()` (o
"requester") — não o formato do manifesto de sync, não a integração com `Vault`/config de fato, não
renderização de QR. Restrição adicional do usuário desde o início ("não quero hardware agora"):
nada de teste contra um celular real — validado só com testes automatizados sem hardware nenhum.

**Implementado**:

- **Novo crate `crates/warden-truthid`** (só lib, sem consumidor ainda — mesma situação em que o
  lado *client* do `warden-server` nasceu): `protocol.rs` (`QrPayload`/`PinResult`, espelham os
  campos do SDK Dart exatamente), `crypto.rs` (fase 1: HKDF-SHA256 sobre o `session_id` cru +
  AES-256-GCM, layout `nonce(12)||ciphertext||tag(16)`; fase 2: ECIES secp256k1 — ECDH + SHA-256
  puro + AES-256-GCM, layout `ephemeral_pubkey(33)||nonce(12)||ciphertext||tag(16)` — replica
  `ecies.dart`/`pin_content_cipher.dart` byte a byte, conferidos linha a linha no código-fonte real
  do TruthID antes de escrever o Rust), `lan.rs` (`candidate_hosts()` via `if-addrs`, varredura de
  `/24` por interface não-loopback × porta fixa `48050-48054`), `requester.rs` (`PendingPin::begin`/
  `qr_payload_json`/`run`, espelha a forma do `PendingRequest` do Dart)
- **`k256` (RustCrypto puro Rust) em vez de `secp256k1`/`libsecp256k1`** — evita mais uma
  dependência nativa pra cross-compilar (o build Android da 7.1 já doeu com isso, `-laaudio` do
  `cpal`), consistente com o resto do ecossistema RustCrypto já usado no projeto
- **`run_with_hosts` como API pública própria**, não só um hack de teste — deixa um chamador que já
  sabe o IP do celular por outro canal pular a varredura; é o que os testes usam pra não varrer a
  LAN real do container (que tem interfaces de verdade, `wlp0s20f3`/`docker0` — varredura completa
  seria lenta e não-determinística num teste automatizado)
- **Testado sem hardware nenhum**: vetor conhecido de RFC 5869 (HKDF-SHA256, Test Case 1) rodado
  direto contra a primitiva usada — prova a primitiva em si, independente de qualquer suposição
  sobre o Dart; round-trip de cada camada de cifra; um "celular fake" (servidor `axum` de teste, só
  dev-dependency, mesmo padrão de `crates/warden-core/tests/mcp_http.rs`) implementando os mesmos
  dois endpoints HTTP single-shot do `RemoteSignerLanServer` real, provando o fluxo `PendingPin`
  inteiro (fase 1 → varredura → fase 2 → decifra ECIES → parse de `PinResult`) de ponta a ponta
- `cargo build/test/clippy --workspace` limpos (9 testes unitários + 1 de integração no crate novo,
  zero regressão no resto do workspace)
- `project/PENDING.md` (P37 novo — visão geral da direção + decisões ainda em aberto; P38 novo —
  lacuna de teste sem hardware; P24 atualizado apontando pra P37), `project/ARCHITECTURE.md`
  (decisão "Sync descentralizado" + decisão de implementação do `warden-truthid`), `project/PHASE.md`
  (nota na Fase 4 apontando pra essa direção nova, etapas antigas de IPFS mantidas até o desenho do
  manifesto ser fechado)

**Ainda falta / decisões em aberto pra fechar a Fase 4 de verdade** (todas registradas em P37):
formato do manifesto de sync (diff tipo-git); como um segundo device descobre "qual é a versão mais
recente" sem tags customizáveis no Arweave; qual identidade deriva a chave de cifra do vault, já
que agora quem paga é o TruthID, não uma carteira do Warden; como agrupar N arquivos mudados num
blob só por sync, já que `pin()` não faz batching. **Nunca testado contra o app TruthID real** —
por pedido explícito do usuário nesta sessão, registrado como P38; maior risco de interoperação é a
convenção exata do "ECDH secret" do pacote Dart `elliptic` (assumida, não confirmada rodando Dart
de verdade).

**Continuação (ainda 2026-09-06, mesma Sessão 50) — troca de stack mobile, Tauri Mobile → Flutter**:
conversa sobre o app mobile do Warden (Fase 7.1, feita na Sessão 49) puxou uma comparação com a
recomendação de Flutter dada em outra sessão pro TruthID — explicado que a diferença vinha do
ponto de partida de cada projeto (TruthID mobile-first sem legado; Warden já tinha desktop em
Tauri, então estender pra mobile reaproveitava UI React + backend Rust sem reescrever nada).
Usuário disse estar "pensando seriamente em trocar" antes que ficasse mais complexo reverter,
priorizando **maturidade geral** e **suporte a iOS** ("acho muito importante"). Analisado que o
timing é bom (só a 7.1 feita, nenhuma etapa de feature ainda) e que a troca não reabre a decisão
de protocolo servidor↔cliente (P1, WS/JSON, mesma sessão) nem o `warden-server` — mobile sempre
foi definido como cliente puro (`PHASE.md`), framework-agnóstico nessa ponta. Usuário confirmou:
**registrar a decisão no `project/` com prioridade**.

**Registrado** (sem código novo, só documentação de decisão): `ARCHITECTURE.md` — linha "Framework
desktop/mobile" separada em "Framework desktop" (Tauri, mantido) e "Framework mobile" (Flutter,
revertido de Tauri Mobile), mais uma seção nova detalhando o raciocínio completo logo após a seção
"Setup Tauri Mobile (Fase 7.1)". `PHASE.md` (Fase 7) — stack atualizada pra Flutter, 7.1 reaberta
(`[ ]`) com nota explicando que substitui o setup anterior. `PENDING.md` — P35 atualizado (era só
o registro dos achados da 7.1) com a decisão de troca e o trabalho que falta, prioridade subida
de 🟡 Baixa pra **🔴 Alta**, a pedido do usuário.

**Ainda não feito**: nenhum código mexido — scaffold Tauri Mobile (`desktop/src-tauri/gen/android/`,
toolchain Android em `~/.local/opt/`) continua no repo, e o projeto Flutter novo ainda não existe.
Fica pra quando o usuário quiser retomar a 7.1 de fato.

**Continuação (2026-09-06, mesma Sessão 50) — retomando P35, 7.1 refeita em Flutter**: usuário
disse "bora continuar?"; apresentadas as opções em aberto (P35 recomeçar 7.1 em Flutter — prioridade
🔴 Alta definida por ele mesmo no fim da sessão anterior —, P37 fechar design do sync Arweave, Fase 9
9.3/9.4, ou 7.2 conectar ao `warden-server`), escolheu **P35**.

**Feito**: scaffold Android do Tauri Mobile removido de vez (`git rm -r desktop/src-tauri/gen/android/`,
`bundle.android.minSdkVersion` tirado do `tauri.conf.json`, resto de `gen/` — build output nunca
versionado — apagado do disco). Flutter SDK stable (3.47.2) instalado sem sudo em
`~/.local/opt/flutter` (clone raso, mesmo espírito do Android SDK da Sessão 49); único obstáculo foi
a falta de `unzip` no sistema — contornado com um shim próprio (`~/.local/bin/unzip`) traduzindo pra
`bsdtar`, sem tocar em pacman/sudo. **Toolchain Android de `~/.local/opt/` inteiro reaproveitado**
(JDK 17, SDK, NDK, licenças já aceitas) — `flutter doctor` confirmou tudo certo sem baixar nada de
novo desse lado. Projeto novo criado em `mobile/` (raiz do repo, fora de `desktop/`) via `flutter
create --org com.warden --project-name mobile --platforms android,ios mobile`
(`applicationId com.warden.mobile`).

**Verificado de ponta a ponta, mesmo rigor da checagem anterior em Tauri Mobile**: `flutter build apk
--debug` compilou de verdade (baixou sozinho NDK r28c e Build-Tools 36 que faltavam), instalado no
mesmo AVD `warden_test` via `adb install`, aberto via `adb shell monkey`, **screenshot real via `adb
exec-out screencap`** confirmando a tela padrão do Flutter renderizando no emulador. Emulador
desligado ao final (`adb emu kill`) pra liberar recursos. `mobile/.gitignore` (gerado pelo próprio
`flutter create`) já cobre build output/`local.properties`/keystores — nada disso versionado.

`project/PENDING.md` (P35 movida pra "Resolvidas" com o resumo completo; P39 nova — lado iOS segue
sem nenhum teste, mesma limitação de sempre, sem Xcode/macOS neste container). `project/PHASE.md`
(7.1 marcada `[x]`). `project/ARCHITECTURE.md` (seção nova "7.1 refeita em Flutter" com todos os
detalhes técnicos — shim do unzip, reaproveitamento do toolchain, verificação via emulador, disco).

**Ainda falta**: nenhuma etapa de feature (7.2-7.6) implementada — só o setup em si. iOS nunca
testado (P39). Layout mobile de verdade (7.3) e conexão ao `warden-server` (7.2) seguem por fazer.

**Próximo passo**: usuário ainda não escolheu entre 7.2 (conectar ao `warden-server`), 7.3 (layout
de chat mobile) ou voltar pra Fase 9/P37 — perguntar por onde seguir.

**Continuação (2026-09-06, mesma Sessão 50) — 7.2, Flutter conecta ao `warden-server`**: usuário
escolheu 7.2. Planejado em modo formal (`/plan`): 2 agentes de exploração em paralelo (protocolo
real do `warden-server` lido direto do código-fonte; estado do scaffold `mobile/` + o que
`PHASE.md`/`ARCHITECTURE.md`/`GUIDELINES.md` já diziam sobre o escopo) + 1 agente de design
(pacotes, camada de protocolo, serviço de conexão, UI, testes — com pushback explícito pedido e
recebido: achou a falta de permissão `INTERNET` no `AndroidManifest.xml` principal, que eu
confirmei lendo o arquivo antes de aceitar). Plano aprovado pelo usuário antes de codar.

**Implementado**: `mobile/lib/protocol/messages.dart` (`ClientMessage`/`ServerMessage` como
`sealed class` do Dart 3.13, espelhando `crates/warden-server/src/protocol.rs` campo a campo,
JSON na mão, sem `json_serializable`/`freezed`); `mobile/lib/services/server_connection.dart`
(espelha `ServerConnection` de `client.rs`, escrito contra `StreamChannel<dynamic>` já que
`WebSocketChannel` já é um); `connection_settings.dart` (`shared_preferences`, texto puro, mesma
postura do OAuth MCP do desktop) + `device_id.dart` (UUID na mão); `screens/connection_screen.dart`
(`StatefulWidget` puro, prefill de host `10.0.2.2` só em debug+Android e só sem valor salvo);
`main.dart` reescrito (saiu o contador demo). `AndroidManifest.xml` ganhou `INTERNET` +
`usesCleartextTraffic`; `Info.plist` ganhou `NSAllowsArbitraryLoads` (não verificável, sem
Xcode/macOS, P39).

**Bug real achado e corrigido durante a implementação** (não previsto no plano): `WebSocketChannel.
stream` e as duas metades de um `StreamChannelController` são *single-subscription* — o plano
original chamava `.stream.first` no handshake e depois `.stream.listen(...)` de novo no modo
conectado, o que teria lançado `Bad state: Stream has already been listened to` em produção assim
que o primeiro `HelloAck` chegasse. Corrigido antes de rodar qualquer teste: uma única
`StreamSubscription` criada antes do `Hello`, callbacks trocados (não re-escutados) na transição
pro modo conectado. Ver `ARCHITECTURE.md` pro detalhe técnico completo.

**Testado**: `flutter analyze` limpo (achou e corrigiu duas dependências transitivas que
precisavam virar diretas — `meta`/`stream_channel`, usadas direto em código de produção).
`flutter test`: 14 testes — 9 de round-trip de protocolo (JSON literal comparado byte a byte
contra o que o Rust produziria) + 4 de `ServerConnection` rodando a lógica REAL de
handshake/heartbeat/goodbye contra um `StreamChannelController` real (não mocks, mesmo espírito de
`crates/warden-server/tests/handshake.rs`) + 1 smoke test de widget. **Verificado de ponta a ponta
contra um `warden-server` real** (`cargo run -p warden-server --listen 0.0.0.0:7420 --auth-key
test-key`, não mockado): app instalado no mesmo AVD `warden_test`, três fluxos confirmados com
screenshot real via `adb` **e** conferidos contra o log do servidor do outro lado — handshake OK
("Connected to warden-server" + log `Android Device (<id>) connected`), `Goodbye` limpo
("Disconnected" + log `<id> said goodbye (Some("user disconnected"))`, confirma o `reason`
chegando intacto), chave errada rejeitada ("Error: authentication rejected: invalid auth key",
igual à mensagem que `client.rs` produziria). `cargo build --workspace` confirmado limpo (nenhum
arquivo Rust tocado).

`project/PHASE.md` (7.2 marcada `[x]`), `project/ARCHITECTURE.md` (seção nova "7.2 — Flutter
conecta ao warden-server" com todos os detalhes, incluindo o bug do single-subscription),
`project/PENDING.md` (P36 atualizado — a lacuna de "sem tailnet real" agora cobre o cliente mobile
também, só testado via `10.0.2.2`).

**Ainda falta**: 7.3 (UI de chat mobile de verdade — a tela atual é só prova de conectividade,
formulário de host/porta/chave), 7.4 (tools locais), 7.5 (push), 7.6 (build/deploy). iOS nunca
testado (P39, inalterado). Sem Tailscale real (P36, atualizado).

**Próximo passo**: perguntar ao usuário se segue pra 7.3 (UI de chat mobile) ou outra frente
(P37 — sync Arweave, ou 9.3/9.4 — registro/roteamento de tool na Fase 9).

---

### 2026-09-05 — Sessão 49

- **Objetivo**: usuário pediu comandos de barra (`/`) no `warden-cli`, no estilo Claude Code —
  `/exit`, e principalmente `/models` (cadastrar/editar/remover/selecionar provedores de modelo) e
  `/agents` (idem pra agentes nomeados/persona), trazendo pro terminal o máximo possível do que a
  Settings do desktop já fazia. Planejado antes de implementar: 3 agentes de exploração em paralelo
  (REPL/config atual, gestão de providers no desktop, gestão de agentes no desktop) + 1 agente de
  design pra validar assinaturas exatas e viabilidade do wizard, plano revisado e aprovado pelo
  usuário antes de codar.

**O que foi feito**:

- **`warden-bootstrap`**: duas funções novas e puras, `rename_provider_cascade`/
  `remove_provider_references` — não existiam em Rust (a lógica de cascade P32/P33 só vivia no
  `SettingsView.tsx` do frontend, que edita um rascunho local só persistido no Save). Como o CLI
  comita cada comando direto no disco, a cascade virou código real, testável sem terminal (3 testes
  novos)
- **`crates/warden-cli/src/commands.rs`** (novo arquivo): parser puro de comando de barra —
  `parse_command`/`Command`/`ParseOutcome`, mais `parse_provider_kind`/`kind_label` (não existia
  `FromStr`/`Display` pra `Provider`). `/foo` desconhecido nunca cai pro chat como mensagem — vira
  cartão de erro. 8 testes novos
- **`interactive.rs`**: `render_input_box`/`read_line` refatorados pra reaproveitar um
  `drive_line_editor` compartilhado (título da caixa parametrizado); `read_field(title, initial)`
  novo, pré-preenche o buffer pro wizard (Enter aceita o valor atual/default, Ctrl+D cancela);
  `run_turn` ganhou `model_override`/`system_prompt`, trocando `handle_message_streaming` por
  `handle_turn_streaming` de verdade; `CliSession` novo guarda só os ids escolhidos
  (`provider_id`/`agent_id`) pela sessão, nunca um objeto resolvido — `resolve_turn_context` relê o
  config do disco na hora só quando alguma seleção está ativa (senão zero overhead). Implementados
  os 13 comandos do plano: `/help`, `/models` (list/use/reset/add/edit/remove),
  `/agents` (list/use/create/edit/remove), cada wizard reaproveitando `read_field` passo a passo
- `cargo build/test/clippy --workspace` limpos — 26 testes em `warden-cli` (18 antigos + 8 novos de
  `commands.rs`), 31 em `warden-bootstrap` (28 + 3 novos de cascade)
- **Verificado via pty com harness próprio** (Python `pty`+parser VT mínimo, config/vault isolados
  em scratchpad, sem tocar `~/.config/warden` real nem gastar cota do Gemini — os comandos de barra
  não fazem chamada de modelo nenhuma): fluxo completo — `/help`, `/models` vazio, `/models add` de
  dois providers (um `openai_compatible` testando o passo de `base_url`, um `gemini` testando os
  defaults pré-preenchidos), listagem com marcador `[ativo]`, `/agents create` com `provider_id`
  apontando pro primeiro, `/agents use`, **`/models edit` renomeando o provider ativo — confirmado
  que `active_provider` e o `provider_id` do agente seguiram o rename** (cascade), **`/models
  remove` do provider renomeado — confirmado que o `provider_id` do agente foi limpo** (cascade de
  delete), e `/exit` encerrando o processo sozinho (status 0, sem precisar matar). `config.toml`
  final inspecionado diretamente batendo com o esperado em cada etapa
- `project/ARCHITECTURE.md` (decisões de design registradas), `project/PENDING.md` (P8 atualizado —
  fecha o eixo "lançamento de agentes" pro lado leve/config-driven)

**Ainda falta / limitações aceitas deliberadamente**: persona de agente é uma linha só (sem
textarea no editor hand-rolled); chave de API no wizard não é mascarada; só o REPL interativo ganhou
os comandos (`run_plain`, usado só por teste via pipe, não). Teste manual numa janela real de
verdade (não só via pty) ainda não confirmado pelo usuário.

**Continuação (ainda 2026-09-05, mesma Sessão 49)**: usuário trouxe vários pedidos de uma vez,
pedindo pra quebrar em partes pequenas e resolver aos poucos — "por enquanto faz só a primeira
parte, commita e da push, registra tudo isso no project". Pedidos, na ordem que vieram:

1. **Tab-completion nos comandos de barra** — "apertar tab e completar /ex entende?", tipo Claude
   Code — **implementado nesta continuação**, ver abaixo
2. **`/usage`** — comando pra ver quanto foi gasto (tokens/custo) na sessão do CLI — **não
   implementado, registrado como continuação de P4 em `PENDING.md`**
3. **Página "home" no app desktop** com dashboards (tokens gastos, custo médio por token, quebra
   por agente, modelos mais usados) e a própria IA com acesso a esses dados — usuário mesmo disse
   "depois complementamos mais essa ideia" — **não implementado, registrado como continuação de P4
   em `PENDING.md`**, precisa de mais definição antes de codar (de onde vêm os dados agregados,
   se existe noção de "custo" em dinheiro ou só tokens, como a IA acessaria)
4. **Trocar os laranjas por roxo no CLI**, pra bater com a identidade visual do app — **implementado
   nesta continuação**, ver abaixo
5. Pergunta direta: "o app vc já mudou pra preto com detalhes em roxo como pedi?" — **respondido,
   não implementado** (não é pedido de código): conferido `desktop/src/App.css` — o tema escuro
   hoje é um roxo bem escuro (`--color-bg: #14101f`), não preto puro, com acentos roxos
   (`--color-accent: #a78bfa`/`#8b5cf6`); tema claro é lavanda claro (`#f8f6fc`). Não achei
   nenhum pedido anterior registrado no `project/` por "preto" — resposta dada ao usuário, sem
   mexer no código ainda (fica pro trabalho de desktop mencionado no pedido 3)

**Implementado (itens 1 e 4)**:

- **Tab-completion**: `commands.rs` ganhou `current_word`/`ghost_suggestion` — dado o que já foi
  digitado depois de `/`, calcula candidatos contra o vocabulário fixo da gramática (nomes de
  comando num nível, de subcomando no próximo) e, quando só sobra um candidato inequívoco, a
  string que falta pra completar. Não tenta completar ids (provider/agent) — precisaria de acesso
  ao config ao vivo, fora de escopo desta parte, registrado como possível segunda etapa
- `interactive.rs`: `render_input_box` ganhou um parâmetro `ghost` — o texto que falta pra
  completar aparece discreto (dim) logo depois do que foi digitado de verdade, na mesma caixa de
  input (sem popup/lista separada). `drive_line_editor` ganhou `suggest_commands: bool` (`true`
  só pro prompt principal do chat, `read_line`; `false` nos campos de wizard, `read_field`, que
  guardam id/persona/chave, não comando) e a tecla `Tab` aceita a sugestão mostrada (insere o
  sufixo + um espaço, pronto pra continuar digitando o argumento)
- `cargo build/test/clippy --workspace` limpos — 32 testes em `warden-cli` (26 + 6 novos:
  `current_word`/`ghost_suggestion`)
- **Verificado via pty**: capturada a tela com `/ex` digitado — o texto completo `/exit` aparece
  (confirma a sugestão fantasma renderizando); e um teste mais forte — `/ex` + Tab + Enter fez o
  processo encerrar sozinho (status 0), confirmando que o Tab de fato completa e o texto resultante
  (`/exit `) é interpretado como o comando de verdade, não só cosmético

**Implementado (item 4 — cor)**:

- As 3 últimas cores laranja do CLI (`Color::Rgb(230, 126, 34)`) — borda da caixa de input, código
  inline no markdown, linha dentro de bloco de código cercado — trocadas por `accent_color()` (o
  mesmo roxo `BRAND`/`#a78bfa` já usado nos cartões e no banner desde a Sessão 47/48). Zero laranja
  restante no `warden-cli`

**Ainda falta**: usuário ainda não testou numa janela real de verdade (nem o tab-completion nem a
cor). `/usage` e o dashboard do desktop ficam pra próximas partes, conforme pedido.

**Continuação (ainda 2026-09-05, mesma Sessão 49) — item 2, `/usage`**: usuário disse "bora
continuar?" — próximo item da fila que ele mesmo definiu era o `/usage`.

**Implementado**:

- `commands.rs`: `Command::Usage`, `("usage", []) => Command::Usage` no parser, e `"usage"`
  adicionado a `TOP_LEVEL_COMMANDS` (ganha tab-completion de graça pelo mecanismo já existente)
- `interactive.rs`: `CliSession` ganhou `usage_total: Usage` e `turn_count: usize` — únicos campos
  da struct sem equivalente no `config.toml` (o resto guarda só ids, relido do disco a cada turno;
  uso é puramente de sessão, nunca persistido, reseta a cada `warden` novo). Acumulados em `run()`
  logo depois de cada `Ok(Some(outcome))` de `run_turn` (mesmo `MessageOutcome.usage` que já
  alimenta o rodapé de tokens da resposta — só passou a também somar num total). `cmd_usage` novo
  renderiza um cartão com contagem de mensagens + tokens de prompt/resposta/total; estado vazio
  ("nenhuma mensagem enviada ainda") antes do primeiro turno. Linha nova no `/help`
- Decisão deliberada: **só tokens, sem `$`** — não existe tabela de preço por modelo no projeto
  ainda (mesma lacuna já registrada em `PENDING.md` P4 pro dashboard do desktop); nem tenta
  fazer conta de custo com dado que não existe
- `cargo build/test/clippy --workspace` limpos (teste de parse do `/usage` adicionado dentro do
  teste existente `exit_and_quit_and_help_parse_with_no_args`, contagem de testes do `warden-cli`
  sem mudar — 32, já que foi extensão de um teste existente, não um novo)
- **Verificado via pty** (harness Python com resposta a `ESC[6n`, já que o script anterior sem
  isso batia no mesmo erro de "cursor position could not be read" descrito no doc comment do
  módulo — faltava emular a query de posição do cursor que um terminal de verdade responde):
  `/usage` antes de qualquer mensagem renderiza o cartão de estado vazio corretamente; `/ex` + Tab
  + Enter ainda completa pra `/exit` e encerra o processo sozinho (confirma que adicionar `/usage`
  não regrediu o tab-completion da Sessão 49 anterior)

**Ainda falta**: acúmulo de tokens de verdade (duas mensagens reais, conferir que a soma bate) não
testado — sem chave de API real disponível no shell do agente, só o roteamento e o estado vazio do
comando foram confirmados via pty. Fica pro usuário confirmar numa janela real com uma chave
configurada. Dashboard do desktop (item 3) segue sem implementar — precisa da definição que o
usuário adiou.

**Continuação (ainda 2026-09-05, mesma Sessão 49) — item 3, dashboard de uso no desktop**: usuário
disse "continua ent" — antes de codar, 3 perguntas pra fechar as decisões que a sessão anterior
tinha deixado em aberto no `PENDING.md` P4 (todas resolvidas com a opção recomendada): fonte dos
dados agregados = agregar sob demanda (sem novo índice persistido); custo em `$` = não, só tokens
por enquanto; acesso da IA aos dados = tool nova (não contexto injetado em toda mensagem).

**Implementado (backend)**:

- `crates/warden-bootstrap/src/usage.rs` (novo arquivo): `UsageSummary`/`UsageByKey` +
  `aggregate_usage(&[Conversation]) -> UsageSummary`, somando `ConversationMessage.usage` de
  toda conversa persistida (lidas via `list_conversations`, já usado pela sidebar do desktop),
  quebrado por `agent_id`/`provider_id`. **Limitação deliberada, documentada no doc comment do
  módulo**: esses dois campos guardam só a *última* seleção de toda a conversa (os seletores do
  desktop), não por mensagem — uma conversa que trocou de provider no meio atribui todo o uso ao
  provider atual, não ao que rodou em cada mensagem de fato; corrigir isso pediria gravar
  provider/agente por `ConversationMessage`, fora de escopo desta agregação sob demanda (a opção
  escolhida sobre um índice persistido novo)
- `UsageStatsTool` (mesmo arquivo) — `Tool` novo, registrado em `bootstrap()` junto de
  `ReadFileTool`/`WriteFileTool` — então **qualquer canal** (CLI, desktop, Telegram, WhatsApp) já
  ganha o modelo respondendo "quantos tokens eu já usei" sob pedido, sem custo de contexto nas
  mensagens que não tocam no assunto (a decisão da pergunta 3)
- `warden-core`: `impl AddAssign<&Usage> for Usage` novo — reaproveitado tanto por `aggregate_usage`
  quanto pelo `/usage` do CLI (parte anterior desta sessão), que perdeu sua função `add_usage`
  solta em favor de `+=`
- `cargo build/test/clippy --workspace` limpos — 5 testes novos em `usage.rs` (soma total ignorando
  mensagem sem uso, quebra por agente/provider ordenada por tokens desc, tool com dir ausente
  retorna `{"error": ...}` em vez de falhar, round-trip do tool lendo conversa salva em disco,
  lista vazia soma zero)

**Implementado (desktop)**:

- IPC `usage_summary` novo (`desktop/src-tauri/src/lib.rs`) — mesma `aggregate_usage`/
  `list_conversations` do backend, lido fresco do disco a cada chamada (nunca cacheado)
- `UsageView.tsx` novo: 5 stat tiles (tokens totais/prompt/resposta, chamadas de modelo,
  conversas) + duas listas de barra horizontal ("By agent"/"By provider"). `AgentEntry.id`/
  `ProviderEntry.id` já dobram como nome de exibição (comentário existente em `types.ts`), então a
  quebra não precisou de nenhum lookup de nome contra Settings
- **Design passou pela skill de dataviz do projeto antes de codar** (carregada explicitamente):
  forma = stat tile pro headline + bar chart pra quebra categórica (não um plot mais pesado —
  poucos pontos, magnitude por categoria); cor = um hue de acento só (o `--color-accent` que já
  existe no app) pras barras, já que cada barra é a mesma métrica (tokens totais) numa categoria
  diferente — identidade vem do rótulo da linha, não da cor, então nenhuma paleta categórica nova
  nem legenda fazem falta aqui; specs de marca seguidos (barra ≤24px, ponta arredondada, valor
  sempre na ponta — "value at the tip" — números grandes em algarismo proporcional nos tiles,
  `tabular-nums` só na coluna de valores das barras). Sem camada de hover: cada barra já mostra seu
  valor via rótulo direto, então um tooltip seria redundante pra uma lista pequena (≤10 categorias)
  dentro de uma tela do próprio app, não um chart publicado à parte
- `Icons.tsx` ganhou `ChartIcon`; `Sidebar.tsx` ganhou um terceiro botão "Usage" no rodapé (ao lado
  de Settings); `App.tsx`/`types.ts` com o roteamento e os tipos (`UsageSummary`/`UsageByKey`)
  espelhando os estruturas Rust campo a campo
- `cargo build/test/clippy --workspace` e `tsc`/`npm run build` do desktop limpos
- **Verificado via Playwright headless contra o dev server real do Tauri** (`npm run dev`, porta
  1420) — não só um harness estático: `window.__TAURI_INTERNALS__.invoke` mockado via
  `page.addInitScript` (a mesma função que `@tauri-apps/api/core`'s `invoke` chama por baixo) pra
  simular `usage_summary`/`get_settings`/`list_conversations` sem precisar de uma janela nativa
  nem de dados reais persistidos no disco. Clique de verdade no botão "Usage" da sidebar (não só
  render direto do componente), tiles e barras renderizando números compactos (`189.6K`, `120K`)
  em claro e escuro, zero erro de console; estado vazio ("No usage recorded yet") também conferido
  com um segundo mock. `playwright` instalado localmente com `--no-save` só pra rodar o teste
  (`package.json`/lockfile do desktop intactos, sem diff) — os navegadores do Chrome for Testing
  tiveram que ser baixados de novo (`npx playwright install chromium`) porque a versão cacheada no
  container não batia com a versão do pacote

**Ainda falta**: teste de ponta a ponta com dados reais numa janela nativa de verdade (não
mockado) — mesma lacuna de sempre, sem chave de API real neste shell pra gerar uso de verdade e
sem confirmação visual do usuário na janela do Tauri em si. Fecha o pedido 3 da lista que o usuário
trouxe nesta Sessão 49 — os 3 pedidos junto com a cor/tab-completion (1 e 4) ficaram todos
resolvidos ao longo da sessão.

**Continuação (ainda 2026-09-05, mesma Sessão 49) — Fase 7.1, App Mobile**: com a fila de pedidos
do usuário esgotada, perguntado "o que fazemos agora?" — resumida a situação (testar o que foi
construído vs puxar o próximo item do roadmap) e o usuário escolheu **"bora pro próximo item do
roadmap"**. Pelo `ROADMAP.md`, isso é a Fase 7 (App Mobile) — a Fase 5/Tools & MCP só tem a 5.4 em
aberto, e essa já está bloqueada pela Fase 8. Antes de começar, avisado que este container só
builda Android (iOS exige Xcode/macOS) — usuário escolheu **instalar o SDK/NDK Android agora** e
seguir de verdade, não só scaffolding.

**Implementado — toolchain e build (detalhes completos em `ARCHITECTURE.md`)**:

- JDK 17 (Temurin), Android cmdline-tools/SDK (platform-tools, platforms 34/36, build-tools 34/35,
  NDK 27) e um `rustup` paralelo (só pros 4 targets Android) instalados **sem `sudo`** em
  `~/.local/opt/` — o Rust do sistema (pacman, usado pelo resto do workspace) ficou intocado
- `cargo tauri android init` gerou `desktop/src-tauri/gen/android/` — commitado (exceto
  `build/`/`.gradle/`/`local.properties`, já cobertos pelo `.gitignore` interno que o próprio
  comando gera), convenção oficial do Tauri
- Primeiro build falhou (`ld.lld: error: unable to find library -laaudio` — o `cpal` de gravação
  de voz nativa, P28, linka `libaaudio.so` incondicionalmente no Android, só disponível a partir
  da API 26 do NDK); corrigido subindo `bundle.android.minSdkVersion` de 24 pra 26 em
  `tauri.conf.json` (não é workaround, é a correção certa — API 26+/Android 8.0+ já cobre a
  esmagadora maioria dos devices ativos em 2026)
- **Disco chegou a 96% de uso na máquina real do usuário** no meio da instalação (`target/` do
  workspace tinha crescido pra 62GB ao longo de sessões anteriores) — avisado o usuário antes de
  agir; escolheu limpar (`cargo clean`, liberou 76GB) em vez de arriscar ou parar por aqui

**Verificado de ponta a ponta com emulador de verdade (não só compilação)**: `cargo tauri android
build --debug --apk` compilado com sucesso pros targets `aarch64` e `x86_64` (o segundo, específico
pra rodar acelerado via KVM — `/dev/kvm` disponível neste container); AVD `warden_test`
(`system-images;android-34;google_apis;x86_64`) criado via `avdmanager`, emulador subido headless,
boot completo em ~68s, APK instalado via `adb install`, app aberto via `adb shell monkey`, e
**screenshot real via `adb exec-out screencap`** confirmando que a UI do React (a mesma do desktop,
zero mudança de código) renderiza dentro do WebView do Android — inclusive a tela de "Usage"
construída na parte anterior desta mesma sessão, visível no rodapé da sidebar.

**Achado real do teste (não um bug — vira trabalho da 7.3)**: a sidebar de largura fixa (280px, CSS
grid `280px 1fr`) praticamente toma a tela inteira numa AVD de 320×640 lógicos — a área de chat
sobra como uma faixa de ~40px. Confirma que a 7.3 ("Interface de chat mobile") precisa mesmo de um
layout responsivo dedicado, registrado como P35 em `PENDING.md` junto com a lacuna do iOS.

`PHASE.md` (7.1 marcada como concluída, nota sobre 7.3 precisar de layout responsivo).

**Ainda falta**: iOS inteiramente não testado (sem Xcode/macOS disponível); a 7.2 (conectar ao
servidor via Tailscale/WebSocket/gRPC) depende do P1 (protocolo servidor↔cliente, ainda decisão em
aberto) — próximo passo natural dentro da Fase 7 seria a 7.3 (layout responsivo mobile) ou voltar
pro P1 pra desbloquear a 7.2.

---

### 2026-09-05 — Sessão 48

- **Objetivo**: usuário testou o fix de raw-mode da Sessão 47 numa janela real e reportou um bug de
  layout novo: "ta tudo cagado, a mensagem está em baixo da caixa de teste, não parece um chat
  normal". Investigar e corrigir.

**O que foi feito**:

- **Causa raiz**: os helpers `write_raw_line`/`write_raw` (introduzidos na própria Sessão 47 pra
  resolver o bug de `\n`/`\r\n`) imprimiam a mensagem do usuário, o cabeçalho "● Warden", a resposta
  e erros direto via `print!`/`println!` em modo raw — sem passar pelo `ratatui`. O `Terminal`
  (`Viewport::Inline`) só sabe onde a caixa embutida está porque ele mesmo atualiza esse
  rastreamento a cada `draw`/`clear`/`insert_before`; um `print!` cru avança o cursor real sem o
  `ratatui` saber, então o próximo `.clear()`/`.draw()` desenhava na posição errada (desatualizada),
  embaralhando a caixa com o texto de conversa já impresso — exatamente o "não parece chat normal"
  relatado
- Lido o código-fonte do `ratatui-core` instalado (`~/.cargo/registry`) pra confirmar a API certa:
  `Terminal::insert_before(height, draw_fn)` é a própria solução do crate pra "imprimir histórico
  permanente acima de uma caixa fixa" — literalmente o caso de uso descrito na doc do método
- **Corrigido**: todo `print!`/`println!` em modo raw trocado por `insert_before`. Como o método
  exige saber de antemão quantas linhas o bloco vai ocupar, `warden-cli/src/interactive.rs` ganhou
  um wrapper de texto próprio (`wrap_text`/`wrap_segment`, usando `unicode_width`) em vez da API de
  contagem de linhas do `ratatui` (que hoje é instável/gated por feature). A resposta em streaming
  passou a acumular num buffer (`pending`): linhas terminadas em `\n` de verdade são promovidas pro
  histórico assim que completam, e qualquer trecho que já preencheu a largura do terminal também é
  promovido linha a linha (a maioria das respostas é um parágrafo contínuo até a quebra final, então
  só esperar por `\n` deixaria o texto invisível até o fim) — a caixinha "Warden respondendo" (nova,
  `render_streaming_box`, verde, substitui a de "pensando" assim que chega o primeiro texto) nunca
  precisa de mais de uma linha de conteúdo, sem precisar redimensionar a caixa. Efeito colateral
  bom: `insert_before` desenha por coordenada de célula, não depende da tradução `\n`→`\r\n` do
  terminal — o bug de "escada" da correção anterior deixa de ser uma preocupação por construção
- `cargo build/test/clippy --workspace` limpos (15 testes em `warden-cli`, `LineEditor` intocado)
- **Verificado com o mesmo harness de pty** (Python `pty`+`select`, mais `pyte` dessa vez pra
  renderizar a tela de verdade em texto legível em vez de bytes crus) — 3 rodadas contra a API real
  do Gemini (não mockada): a linha "> mensagem" e um erro real de várias linhas (JSON do Gemini, 503
  "high demand") renderizaram corretamente coladas acima da caixa, sem sobreposição nem escada, nas
  três rodadas — a caixa ficou fixa embaixo, como esperado
- **Limitação**: o Gemini esteve instável durante toda a verificação desta sessão (503 "high demand"
  repetido, uma chamada passou de 70s sem responder nem errar) — não foi possível ver uma resposta
  de texto real (só o caminho de erro) renderizando em streaming de ponta a ponta; a lógica de
  promoção linha-a-linha do `pending` ficou coberta só pelo raciocínio + os testes automatizados
- `PENDING.md` (P34) atualizado com a causa raiz e o fix, mantido em aberto até o usuário confirmar
  com uma resposta de verdade quando o Gemini normalizar

**Próximo passo**: usuário testar numa janela real com uma mensagem simples assim que a API do
Gemini estiver respondendo normalmente, prestando atenção especial ao texto da resposta em si
(não só o erro) renderizando em streaming, linha a linha, sem travar nem embaralhar.

**Continuação (ainda 2026-09-05, mesma Sessão 48)**: usuário testou o fix do layout de verdade —
confirmou que melhorou, mas "ainda não está um Claude Code da vida". Perguntado o que
especificamente, apontou (múltipla escolha + texto livre): falta cor/formatação no texto (markdown
cru), o visual dos "balões"/mensagens não tem identidade clara, a caixa de pensando/status ainda
incomoda, e de forma geral "como as mensagens ficam estruturadas, tá mt esquisito".

**Implementado nesta continuação**:

- **Markdown inline** (`parse_inline`, novo) — `**negrito**`, `*itálico*`/`_itálico_` e `` `código` ``
  extraídos de uma linha e convertidos em spans estilizados (`Vec<(String, Style)>`); não é
  CommonMark completo, só os construtos que uma resposta de chat realmente usa. Um marcador aberto
  no fim do texto disponível (ainda sem o par de fechamento) fica como caractere literal — não tenta
  "adivinhar" o fechamento
- **Classificação de bloco** (`classify_and_strip`, novo) — reconhece cabeçalho (`#`/`##`/`###`),
  item de lista (`-`/`*`) e bloco de código cercado (` ``` `, com um `bool in_code_block` mantido
  pelo chamador ao longo do turno inteiro, já que uma linha de cerca sozinha não deve renderizar
  nada). Cabeçalho vira negrito + cor de destaque (a mesma roxa do `BRAND`/banner); item de lista
  ganha um marcador "• " roxo; linha dentro de um bloco de código fica na cor de destaque de código
  (a mesma laranja que já era usada pro código inline), sem parsing de markdown dentro (senão
  `**`/`` ` `` dentro de código de verdade virariam negrito/código por engano)
- **Wrap com estilo preservado** (`wrap_spans`, novo) — mesmo algoritmo guloso do `wrap_segment` já
  existente, mas operando sobre spans já estilizados em vez de texto plano, pra negrito/itálico
  sobreviverem à quebra de linha em vez de virar texto plano de novo
- **Estrutura de "hanging indent" por mensagem** (`insert_history_rows`/`commit_assistant_rows`,
  novos) — a primeira linha de uma mensagem inteira (do usuário ou do Warden) ganha o marcador
  ("> "/"● "), toda linha seguinte (inclusive quebras de parágrafo dentro da mesma resposta, ao
  longo de várias chamadas separadas de `insert_before` numa resposta longa) ganha só um recuo de
  dois espaços — o pedido concreto de "estrutura esquisita": antes cada linha impressa não tinha
  relação visual nenhuma com as outras da mesma mensagem
- **Eco da mensagem do usuário** — trocado de negrito laranja pra um "> " discreto/dim, sem cor —
  o orçamento de cor fica reservado pro markdown da resposta do Warden, não pra repetir o que o
  próprio usuário acabou de digitar (mais parecido com como um chat normal trata o próprio input)
- **Caixa de pensando/status sem borda** (`render_thinking_line`/`render_streaming_line`, era
  `render_thinking_box`/`render_streaming_box`) — a caixa com borda ficou só pro input de texto (que
  o usuário já tinha elogiado antes, não mexida); o status "pensando"/"respondendo" virou uma linha
  solta sem `Block`, desenhada na mesma linha vertical de antes (meio das 3 linhas do viewport
  embutido) pra não pular visualmente ao trocar de um pro outro
- **Limitação conhecida, aceita deliberadamente**: como a maioria das respostas ainda precisa da
  promoção linha-a-linha antes do parágrafo terminar (senão o texto fica invisível até o fim, o
  problema que a correção anterior resolveu), a classificação de bloco (cabeçalho/lista/código) só
  roda quando uma linha termina de verdade com `\n` — o trecho ainda em streaming (sem `\n` ainda)
  usa só o parsing inline, sem marcador de bloco. Isso significa que um marcador de markdown (`**`,
  `` ` ``, cabeçalho/lista) que calhe de ficar dividido bem na fronteira entre duas promoções
  separadas pode deixar um asterisco ou marcador solto visível — confirmado numa das rodadas de
  teste (ver abaixo), efeito raro e cosmético só, sem quebrar o layout
- `cargo build/test/clippy --workspace` limpos — 6 testes novos em `warden-cli`
  (`parse_inline_extracts_bold_italic_and_code_spans`,
  `parse_inline_leaves_an_unclosed_marker_as_literal_text`,
  `classify_and_strip_recognizes_headers_and_bullets`,
  `classify_and_strip_toggles_code_block_state_across_calls`,
  `wrap_spans_breaks_on_word_boundaries_and_preserves_style`,
  `wrap_spans_hard_breaks_a_single_word_longer_than_the_width`), 21 no total no crate
- **Verificado com o mesmo harness de pty (Python `pty`+`pyte`)**, desta vez com uma chamada real ao
  Gemini que **respondeu de verdade** (não só erro) — pedido "liste 3 dicas de produtividade em
  bullets, cada uma com uma palavra em negrito": confirmado visualmente (dump de tela + dump de
  estilo por célula) — "● " verde só na primeira linha da resposta, "• " roxo em cada item de lista,
  negrito de verdade em "Foco"/"Priorização", linhas de continuação recuadas sem repetir o marcador,
  eco do usuário discreto/dim, caixa de pensando sem borda. Uma ocorrência do limite conhecido acima
  apareceu (um asterisco solto onde um itálico ficou dividido entre duas promoções), consistente com
  o esperado — nada mais quebrado

**Próximo passo**: usuário testar numa janela real e dizer se a estrutura/cor está no nível esperado
agora, ou se ainda falta algo específico.

**Continuação (ainda 2026-09-05, mesma Sessão 48)**: usuário testou de novo numa janela real,
colou o resultado (mensagem + erro 503 renderizando certo, confirmando o fix de layout) e disse:
"ta simples dms, não tem como deixar mais bonitão, com mais detalhes e tudo mais?". Apresentadas 3
opções visuais em ASCII (cartões com borda por mensagem / sem caixa mas com metadados / só a
resposta do Warden em card) — usuário escolheu **cartões com borda por mensagem**.

**Implementado**:

- `insert_history_rows`/`commit_assistant_rows` (do "hanging indent" da rodada anterior) removidos
  — trocados por `insert_card`, que desenha um cartão inteiro (borda de cima com título embutido,
  linhas de conteúdo já quebradas/estilizadas emolduradas por "│ "/" │", rodapé opcional com
  espaçador antes, borda de baixo) numa única chamada de `insert_before` — uma borda não pode ser
  "reaberta" depois de fechada pra receber mais linhas, diferente do esquema anterior de ir
  promovendo linha a linha
- Isso mudou a estratégia de streaming: como um cartão só é commitado quando está completo, o texto
  cru da resposta só vai acumulando (`pending`) enquanto chega, mostrado ao vivo numa
  pré-visualização com a mesma borda arredondada (`render_card_preview`, título "● Warden") que
  **cresce dinamicamente** até um teto de 6 linhas (`MAX_PREVIEW_ROWS`, depois disso só mostra a
  cauda) — só quando a resposta termina de verdade é que o texto completo passa pelo parser de
  markdown (`markdown_body_rows`) de uma vez só e vira um cartão permanente. Isso fecha de vez a
  limitação conhecida da rodada anterior (marcador de markdown dividido entre duas promoções
  separadas), já que agora só existe UMA promoção, com o texto inteiro já disponível
- A pré-visualização dinâmica exigiu voltar a reconstruir o `Terminal` em tempo de execução
  (`ensure_preview_height`/`new_inline_terminal_with_height`) — a mesma operação que causou o
  travamento da Sessão 47 com o `EventStream` antigo. Agora é seguro: o `EventStream` já foi
  removido de vez naquela sessão (só restam `poll`/`read` síncronos e limitados), então reconstruir
  o `Terminal` só faz uma query de cursor pontual, sem risco do lock ficar preso
- Mensagem do usuário também virou cartão (`plain_body_rows` — sem parsing de markdown, é texto
  literal dele), título "você" discreto/dim, borda dim; erro também virou cartão, com borda
  vermelha. A caixa de input ganhou borda arredondada (`BorderType::Rounded`) pra combinar
  visualmente com os cartões novos (antes usava o canto reto padrão do `ratatui`)
- `cargo build/test/clippy --workspace` limpos — mesmos 21 testes (as funções puras de
  parsing/wrap não mudaram; `insert_card`/`ensure_preview_height` não são testáveis sem terminal
  de verdade)
- **Verificado via pty**: cartão do usuário e pré-visualização com borda arredondada renderizando
  certo; bateu num rate limit do Gemini de novo (429 por minuto dessa vez, `retryDelay: 57s`, não a
  cota diária) — o cartão de ERRO em si renderizou certinho (borda vermelha, JSON de várias linhas
  emoldurado e alinhado), mas grande o bastante (~30 linhas) pra estourar a altura do terminal de
  teste (34 linhas) e rolar o título/borda de cima pra fora da tela capturada — artefato do teste
  (terminal pequeno), não um bug, já que o desenho da borda de cima é idêntico ao já confirmado
  funcionando na rodada anterior

**Ainda falta**: nunca visto uma resposta de verdade (não só erro) completa no novo formato de
cartão — toda tentativa de hoje bateu em rate limit do Gemini antes de completar. Usuário pediu
pra deixar assim por enquanto ("deixa assim por enquanto, só atualiza o project... e commita e da
push") — retomar o teste de ponta a ponta quando o Gemini normalizar.

**Continuação (ainda 2026-09-05, mesma Sessão 48)**: usuário, enquanto ainda esperava a cota do
Gemini normalizar, notou que os cartões de mensagens pequenas ("oi", "ok") ficavam esticados até a
borda do terminal igual um card de resposta longa — perguntou se dava pra encolher o cartão pro
tamanho da própria mensagem.

**Implementado**:

- Extraída a lógica de largura de `insert_card` pra uma função pura nova, `card_width(title_width,
  content_rows, max_width)` — calcula a largura pela linha mais longa do título/conteúdo, com piso
  de 12 colunas e teto na largura real do terminal. Como o corpo já vem pré-quebrado por
  `markdown_body_rows`/`plain_body_rows` respeitando o teto do terminal (`card_content_width()`),
  encolher depois com base na linha mais larga de verdade nunca força re-quebra — só estreita
- 2 testes novos (`card_width_shrinks_to_fit_a_short_message_instead_of_the_full_terminal`,
  `card_width_grows_up_to_its_widest_row_but_never_past_max_width`) — 23 testes no crate no total.
  `cargo build/test/clippy --workspace` limpos
- **Verificado via pty**, dessa vez com um harness próprio em vez do de sessões anteriores (não
  achei `pyte` instalado e o ambiente é Arch "externally managed" — sem `pip`, não instalei nada
  sem pedir permissão): um emulador VT mínimo em Python (`pty.fork` + parser manual de `CSI`,
  respondendo à query de posição do cursor `ESC[6n` que o `ratatui` faz, senão trava igual ao bug
  já corrigido da Sessão 47) escrito no scratchpad da sessão, não commitado. Confirmado: "oi"
  virou um cartão de ~12 colunas; o cartão de erro (JSON longo do 429 do Gemini, real, não mockado)
  continua esticado até a borda — os dois lado a lado na mesma tela, exatamente o comportamento
  esperado

**Ainda falta**: mesma pendência de antes — nenhuma resposta completa (não-erro) vista no formato
de cartão novo, por causa do rate limit do Gemini seguindo ativo.

---

### 2026-09-04 — Sessão 47

- **Objetivo**: usuário testou o CLI `ratatui` da Sessão 46 (P34) numa janela real pela primeira
  vez — travou em "pensando" sem nunca voltar ("ta terrivel, mandei e ficou pensando e não para,
  tirando que ta mt feio"). Investigar e corrigir a causa raiz.

**O que foi feito**:

- Reproduzido o travamento sem precisar do usuário: harness em Python (`pty.fork` + `select`)
  simulando um terminal real, incluindo resposta às queries de posição do cursor (`ESC[6n`) que um
  terminal de verdade responderia — sem isso, o processo já falhava na primeira query, escondendo
  o bug real
- **Causa raiz**: `ratatui` faz uma leitura síncrona e bloqueante de `ESC[6n` toda vez que o
  `Terminal` é construído (`Terminal::with_options`) ou que `.clear()` é chamado — usado a cada
  turno pra apagar a caixa de "pensando" antes de imprimir a resposta. Essa leitura e o
  `EventStream` assíncrono do crossterm disputam o mesmo lock global interno do crossterm
  (`INTERNAL_EVENT_READER`): a thread de fundo que o `EventStream` cria, uma vez usada, fica
  bloqueada segurando esse lock indefinidamente enquanto espera a próxima tecla — o estado normal
  durante um turno, já que ninguém digita enquanto o modelo responde. Resultado: a query de cursor
  nunca consegue o lock e estoura o timeout fixo de 2s do crossterm, toda vez que não há tecla
  sendo pressionada. É um comportamento **documentado no próprio crossterm** (doc-comment de
  `cursor::position()`: "this function will block and possibly time out while `event::read`/`poll`
  are being called"), não uma falha isolada desta implementação
- **Corrigido**: `EventStream` removido de vez do `warden-cli` — trocado por
  `crossterm::event::{poll, read}` síncronos com timeout curto (30ms), que nunca seguram o lock
  além do próprio timeout, então a query de cursor sempre encontra ele livre. Um único `Terminal`
  agora é construído uma vez por sessão (não mais um novo por turno, que já era uma causa
  secundária do mesmo tipo de disputa) e reaproveitado por `read_line`/`run_turn`.
  `run_turn` deixou de usar `tokio::select!` (não faz mais sentido sem `EventStream`) — vira um
  loop que drena o canal de eventos sem bloquear, redesenha o spinner e faz o poll limitado de
  teclado a cada iteração, dobrando como o "tick" antigo. Dependência `futures-util` e a feature
  `event-stream` do `crossterm` saíram do `Cargo.toml` do `warden-cli` por não serem mais usadas
- Verificado com o mesmo harness de pty, incluindo uma chamada real (não mockada) à API do Gemini
  configurada no `~/.config/warden/config.toml` do usuário: turno completo funcionando de ponta a
  ponta — caixa de "pensando" por ~10-13s (latência real do Gemini observada nesta sessão),
  transição limpa pra "● Warden" + texto da resposta, contagem de tokens, volta pra caixa de input
  vazia, sem travar. `cargo build/test/clippy --workspace` limpos (15 testes no `warden-cli`, todos
  os outros crates sem mudança nenhuma)
- **Efeito colateral notado durante os testes**: o free tier do Gemini (`gemini-3.5-flash`) tem um
  limite de 20 requisições/dia por projeto — as várias chamadas reais feitas pra diagnosticar e
  confirmar o fix provavelmente esgotaram a cota diária da chave do usuário (erro 429
  `RESOURCE_EXHAUSTED` visto no fim dos testes). Não é um bug do Warden — o app tratou o erro
  graciosamente (mensagem clara, volta pro prompt) — mas o usuário pode encontrar isso ao testar
  de novo hoje
- `PENDING.md`: P34 atualizado com a causa raiz e o fix, mantido em aberto — falta a confirmação
  visual do próprio usuário numa janela real (o pty sintético prova que não trava mais, mas não é
  a mesma coisa que ver renderizado), e a reclamação de "tá muito feio" (mesma mensagem do usuário)
  segue sem detalhe do que especificamente incomoda, não endereçada nesta sessão

**Próximo passo (revisado depois do usuário responder ao pedido de detalhe, ainda na Sessão 47)**:
usuário detalhou o "muito feio" — três pedidos concretos: (1) início limpo tipo Claude Code, sem
os comandos antigos do shell aparecendo atrás do banner; (2) melhorar especificamente "a parte
superior" (o banner, que hoje era texto puro sem cor nenhuma); (3) responsivo ao tamanho da janela.
A caixa de input em si foi elogiada como já estando boa — não mexida.

Implementado na sequência, ainda nesta sessão:

- **(3) confirmado que já funcionava** antes de qualquer mudança nova — verificado via pty
  redimensionando de 120 pra 80 colunas em pleno uso: a borda encolheu corretamente no próximo
  frame, sem travar. `ratatui` já revalida o tamanho do terminal (via `ioctl`, não a query de
  cursor problemática de antes) a cada `Terminal::draw()` — não precisou de código novo
- **(1) `clear_screen()` novo** — `\x1b[2J\x1b[3J\x1b[H` (limpa tela visível **e** scrollback, home
  no cursor) chamado logo no início de `run()`, antes de qualquer print, inclusive antes do próprio
  banner. Como isso limpa o scrollback inteiro do terminal (não só a área visível), os avisos de
  bootstrap ("TAVILY_API_KEY not set", "shell tool disabled") impressos *antes* de
  `interactive::run` ser chamado também somem da tela — o usuário só vê o banner do Warden ao
  abrir, igual pedido
- **(2) banner redesenhado** — usa a mesma cor de marca roxa que o app desktop já tem
  (`--color-accent` em modo escuro do `desktop/src/App.css`, `#a78bfa` / `rgb(167, 139, 250)`) em
  vez do laranja/amarelo escolhido meio ao acaso nas sessões anteriores só pro CLI; "Warden" em
  negrito roxo, subtítulo continua discreto (`dimmed`), e uma linha divisória fina abaixo, também
  roxa e discreta, com a largura real do terminal (via `crossterm::terminal::size()`, não um número
  fixo chutado) — separa visualmente o cabeçalho da conversa que cresce abaixo
- Verificado via pty (sem gastar mais cota do Gemini — só até a caixa de input aparecer, sem
  mandar mensagem) que a sequência de escape do clear roda antes do banner e que as cores/divisória
  aparecem certas. `cargo build/test/clippy --workspace` limpos

**Próximo passo (revisado de novo depois do usuário testar, ainda Sessão 47)**: usuário testou —
confirmou que era mesmo o 429 (cota diária do Gemini esgotada, como avisado), mas reportou um bug
novo: o texto do erro "ficou todo jogado pela tela, embaixo, em cima do campo de digitar, e todo
picotado".

**Causa raiz**: modo raw (`cfmakeraw`, o que `crossterm::terminal::enable_raw_mode` liga) desliga o
processamento de saída do terminal (`OPOST`) — inclusive a tradução automática de `\n` sozinho pra
`\r\n` que todo terminal faz por padrão fora do modo raw. Isso significa que **todo** `println!`
chamado depois do raw mode ligado (o eco "> mensagem", o cabeçalho "● Warden", a linha de tokens, a
mensagem de erro) deixava o cursor onde a linha anterior tinha parado, em vez de voltar pra coluna
0 — inofensivo pra uma linha só (não dava pra notar), mas o corpo do erro 429 do Gemini é um JSON
de várias linhas, deixando o efeito bem visível: cada linha nova começava mais à direita que a
anterior, "escada" espalhada pela tela, exatamente o relato do usuário. Esse mesmo bug atingiria
qualquer resposta do modelo com múltiplos parágrafos, não só mensagens de erro — só não tinha
aparecido ainda nos testes anteriores porque as respostas de teste ("Hello"/"Hi") eram sempre uma
linha só

**Corrigido**: dois helpers novos, `write_raw_line`/`write_raw` (`interactive.rs`) — trocam
qualquer `\n` (inclusive os embutidos no meio do texto, não só o final) por `\r\n` antes de
imprimir. Todo `println!`/`print!` que roda depois do raw mode ligado (dentro de `run_turn` e no
loop principal de `run`) migrou pra eles; o banner e o `clear_screen()` continuam com `println!`
normal porque rodam **antes** do raw mode ligar, onde a tradução automática do terminal ainda
funciona. Verificado via pty forçando o mesmo erro 429 de propósito (reprodutor perfeito, por ser
JSON de várias linhas): zero `\n` sozinho no texto do erro depois do fix — os únicos dois `\n` sem
`\r` que sobraram no capture inteiro são do próprio `ratatui` reservando espaço pro viewport
(linhas em branco, sem texto depois pra desalinhar, inofensivas). `cargo build/test/clippy
--workspace` limpos

**Próximo passo**: usuário confirmar visualmente numa janela real — em especial mandar uma
mensagem que dê uma resposta de verdade (não só o erro 429) pra ver o texto de várias linhas
renderizando limpo, já que a cota do Gemini deve seguir zerada por hoje.

---

### 2026-09-04 — Sessão 46

- **Objetivo**: usuário pediu um `/code-review high` focado no app desktop (`desktop/`); na
  sequência, pediu pra retomar P8 — deixar o `warden-cli` "com cara de Claude Code, mas mais
  bonitinho e menos focado em código".

**O que foi feito**:

- Rodado `/code-review high desktop` (fork em background) — achou uma lacuna real no mesmo
  espírito do fix de P32 (Sessão 45): `updateProvider` já propaga rename de provider pro
  `providerId` de agentes que o referenciam, mas `deleteProvider` não fazia o equivalente pro
  caso de exclusão — só limpava `activeProvider`. `save_settings` (`lib.rs`) validava só
  `active_provider` contra a lista de providers, nunca `agents[].provider_id`, deixando a
  referência pendurada ser salva sem erro. O agente verificador do review achou um terceiro
  ponto: `App.tsx::handleSelectAgent` aplicava `agent.providerId` sem o guard de existência já
  usado em `App.tsx:60-61` pra restaurar provider de conversa
- Aplicados os três fixes, espelhando o padrão já validado pro rename: `deleteProvider`
  (`SettingsView.tsx`) zera `providerId` de agentes órfãos; `save_settings` (`lib.rs`) ganhou
  validação de `agents[].provider_id`, recusando o Save com erro claro em vez de persistir
  silencioso; `handleSelectAgent` (`App.tsx`) ganhou o mesmo guard de existência antes de aplicar
  o provider padrão do agente
- `cargo build/test/clippy --workspace` e `tsc`/`npm run build` (desktop) limpos
- `PENDING.md`: nova pendência **P33**, já registrada direto em "Resolvidas" (achada e corrigida
  na mesma sessão)

**P8 — CLI com streaming real + `ratatui`**: perguntado o que "cara de Claude Code" envolvia
concretamente, usuário confirmou querer os quatro itens juntos (caixa de input com borda, banner,
streaming de resposta de verdade, barra de status com tempo decorrido) e, avisado do custo real
(mudar `ModelProvider`/os 3 providers, reabrir a decisão de 2026-08-09 de não usar `ratatui`),
escolheu ir fundo mesmo em vez da versão leve só-no-CLI. Plano desenhado com apoio de um agente de
design (validou o desenho da trait e o mapeamento de eventos SSE por provider, inclusive a
descoberta de que o `FunctionCall.args` do Gemini nunca chega picotado, ao contrário de
OpenAI/Anthropic) antes de qualquer código.

- `crates/warden-core/src/model/mod.rs`: `ModelProvider` ganhou `chat_stream` como único método
  obrigatório; `chat()` virou um método **default** que drena o stream (`drain_chat_stream`,
  `pub(crate)`, compartilhado com o orchestrator) — resultado prático: Telegram, WhatsApp, Desktop
  (`send_message`) e `DelegateTool` **não mudaram nenhuma linha de código de produção**, só os 7
  mocks de teste trocaram `chat` por `chat_stream` (mecânico, via `response_stream(Response) ->
  ChatStream` novo). `StreamEvent` novo (`ContentDelta`/`ToolCallDelta`/`Usage`)
- `openai.rs`/`anthropic.rs`/`gemini.rs`: streaming de verdade via SSE (`eventsource-stream` sobre
  `reqwest::bytes_stream()`, `async-stream` pra montar cada `Stream`). Gotcha real corrigido:
  OpenAI exige `stream_options.include_usage: true` ou o `usage` some em modo streaming; Anthropic
  reporta usage partido em dois eventos (`message_start`/`message_delta`), combinados antes de
  emitir. Gemini trocou pra `:streamGenerateContent?alt=sse` — como `args` é objeto JSON nativo no
  wire (nunca string), o provider emite um `ToolCallDelta` inteiro de uma vez por function call,
  sem precisar de nenhum caso especial no acumulador. Testes novos por provider (parsing de SSE
  fabricado direto, sem HTTP) incluindo o teste de regressão da suposição do Gemini
- `orchestrator/mod.rs`: `handle_turn` virou wrapper de `handle_turn_streaming` novo (uma só
  implementação do loop de tool-calling, não duas que pudessem divergir); `handle_message_streaming`
  novo é o único ponto de entrada que o CLI usa de verdade. Testes novos provam paridade com o
  comportamento de antes de existir streaming, mais ordem dos eventos e erro no meio do stream
- `crates/warden-cli/src/interactive.rs` reescrito: `ratatui` em modo `Viewport::Inline` (não
  fullscreen — scrollback do terminal continua normal, ao contrário de um TUI de tela alternativa)
  só pra caixa de input com borda e caixa de "pensando" (spinner + tempo decorrido + Ctrl+C pra
  interromper); `rustyline` saiu, entrou um `LineEditor` próprio (cursor UTF-8-aware, histórico com
  draft), testado isolado sem terminal nenhum (10 testes novos). Resposta impressa como texto puro
  conforme chega assim que o primeiro delta aparece (sem markdown ao vivo — trade-off deliberado,
  documentado em `ARCHITECTURE.md`). Ctrl+C aborta a chamada em andamento sem gravar resposta
  parcial no histórico da conversa
- `cargo build/test/clippy --workspace` limpos (48 testes no `warden-core`, 15 no `warden-cli`,
  zero warning novo em nenhum crate, incluindo Desktop/Telegram/WhatsApp/MCP-server que não
  precisaram de nenhuma mudança de código)
- `PENDING.md`: P8 atualizado (não fechada — fica aberta a parte de "lançamento de agentes"), nova
  pendência **P34** pro teste de ponta a ponta numa janela real (mesma limitação de sempre pra
  automatizar terminal raw-mode a partir daqui). `ARCHITECTURE.md`: duas decisões novas

**Próximo passo**: usuário confirmar visualmente numa janela real (P34) — abrir `cargo run -p
warden-cli`, digitar, checar a caixa de borda, histórico ↑/↓, Ctrl+C interrompendo um streaming em
andamento, e a transição da caixa de "pensando" pro texto da resposta chegando aos poucos. Sem
outra pendência de UX geral aberta além do que já estava em `PENDING.md` antes desta sessão
(P24/P29/P30/P31).

---

### 2026-09-04 — Sessão 45

- **Objetivo**: Continuar de onde a Sessão 44 parou — puxar P32 (teste de ponta a ponta dos
  agentes nomeados + seletores por conversa da Sessão 43, ainda sem chave de API real disponível
  até então). Usuário topou testar ao vivo.

**O que foi feito**:

- Subi o app desktop de verdade (`npm run tauri dev`, build limpo) pro usuário testar na janela
  nativa. Confirmado de saída: não dá pra automatizar clique/teclado nela por aqui — é um cliente
  Wayland nativo (KDE/KWin), `xdotool` (só XWayland) nem enxerga a janela — mesma limitação já
  registrada em sessões anteriores. Teste ficou por conta do usuário, com acompanhamento aqui
- Usuário cadastrou uma chave Gemini real em Settings e bateu em dois bugs reais em sequência,
  ambos investigados e corrigidos na hora (detalhes em `ARCHITECTURE.md`):
  1. **`active_provider` órfão** — renomear o "Name" de um provider em Settings edita `provider.id`
     direto, mas nada sincronizava `activeProvider`/`agent.providerId` com o rename. O usuário criou
     um provider (id automático `provider-1`), renomeou pra `gemini`, `active_provider` ficou preso
     no nome antigo — `save_settings` aceitava sem validar, só quebrava depois ao mandar mensagem
     (`active_provider 'provider-1' not found among configured providers`). Corrigido nos dois
     lados: `SettingsView.tsx` (`updateProvider` agora propaga o rename) e `desktop/src-tauri/src/
     lib.rs` (`save_settings` ganhou validação nova — recusa um `active_provider` que não bate com
     nenhum provider da lista, com erro claro no Save em vez de só no envio)
  2. **Gemini "thinking" rejeitando `functionCall` sem `thoughtSignature`** — depois do fix acima,
     a mensagem chegou de verdade na API (confirmado por um 503 transiente do lado do Google, não
     bug nosso), mas a segunda tentativa deu 400 INVALID_ARGUMENT assim que o modelo chamou
     `delegate_task`: `gemini-3.5-flash` é um modelo "thinking" que anexa um `thoughtSignature`
     opaco à `Part` de um `functionCall` e exige o mesmo valor de volta no turno seguinte — o
     `ToolCall` compartilhado (`warden_core::model`) não carregava esse dado, então nenhuma
     chamada de tool sobrevivia a mais de um turno contra o Gemini. Corrigido: `ToolCall` ganhou
     `thought_signature: Option<String>` (sempre `None` pra OpenAI/Anthropic), `GeminiProvider`
     captura o campo (`ResponsePart.thought_signature`, sibling de `functionCall`, não aninhado) e
     reenvia no `Part` reconstruído em `to_content()`. 2 testes novos em `gemini.rs`
- Usuário confirmou: reenviou a mensagem depois dos dois fixes, a resposta refletiu a persona do
  agente "pirata" criado em Settings — P32 fechado
- `cargo build/test/clippy --workspace` limpos (30 testes na lib de `warden-core`, incluindo os 2
  novos do Gemini; nenhum teste quebrou apesar do campo novo em `ToolCall`, que exigiu atualizar 6
  sites de construção — 3 em código de produção, 3 em mocks de teste/`pipeline.rs`); `tsc` limpo.
  App recompilou sozinho via o watcher do `tauri dev` durante as edições
- `project/PENDING.md` (P32 movido pra "Resolvidas", descrevendo os dois bugs achados no processo),
  `project/ARCHITECTURE.md` (2 decisões/fixes novos), `project/SESSIONS.md` (esta entrada e a 44,
  retroativa — o commit de backlog de voz da sessão anterior nunca tinha ganhado uma entrada aqui)

**Próximo passo**: nenhuma pendência de UX geral aberta além do que já está em `PENDING.md`
(P29/P30/P31 — testes de imagem/voz de ponta a ponta, adiados sem prioridade; P24 — Fase 4 precisa
repensar IPFS→Arweave). Pelo `ROADMAP.md`, o próximo item grande na ordem combinada é **App Mobile**
(Fase 7).

---

### 2026-09-04 — Sessão 44

- **Objetivo**: Usuário perguntou se dava pra usar outros provedores além da OpenAI pra voz
  (STT/TTS) — pergunta exploratória, não pedido de implementação.

**O que foi feito**:

- Expliquei o estado atual: `transcribe.rs`/`speech.rs` (Sessões 41/42) estão fixos na OpenAI
  (Whisper/`tts-1`), independente de qual provider de chat está ativo — diferente do registry de
  `ModelProvider`, que já é plugável
- Levantadas as alternativas, sem decisão de prioridade: Gemini nativo (já aceita áudio de entrada
  e tem TTS próprio, reaproveitaria a chave já cadastrada), `whisper.cpp` local (STT sem
  custo/chave, mais privado, mas sem TTS local equivalente), provedor dedicado de terceiros (ex.
  ElevenLabs, quando qualidade de síntese importa mais)
- Usuário optou por não implementar agora — registrado como ideia de backlog em `ROADMAP.md`
  ("Voz plugável além da OpenAI"), sem pendência nova em `PENDING.md` (é uma ideia levantada, não
  uma lacuna encontrada numa feature já implementada)

**Próximo passo**: nenhum, decisão foi só registrar. Usuário retomou pedindo pra continuar o
projeto na sessão seguinte (Sessão 45).

---

### 2026-09-03 — Sessão 43

- **Objetivo**: usuário pediu pra melhorar o app — perguntou como estava a gestão/criação de
  memória hoje, e pediu pra poder criar múltiplos agentes nomeados com uma personalidade
  configurável (texto livre), selecionáveis por conversa igual o modelo já podia ser selecionado
  globalmente na Settings — e que o modelo também passasse a ser trocável por conversa, não só
  globalmente. Fecha P3 (formato do prompt de sistema/persona), em aberto desde a Fase 1.

**O que foi feito**:

- Expliquei o estado atual da memória antes de implementar: o `Vault` é só uma pasta de markdown
  em disco (compatível com Obsidian), busca por substring simples, duas tools (`read_file`/
  `write_file`) que o modelo pode chamar — sem UI no desktop pra navegar/criar memórias
  diretamente
- Duas decisões de produto confirmadas com o usuário antes de implementar: agentes vivem no
  `config.toml` como um registry (mesmo padrão dos providers, Sessão 35), não como arquivos no
  vault; trocar agente/modelo no meio de uma conversa afeta só as mensagens daí pra frente
- `crates/warden-core/src/orchestrator/mod.rs`: `handle_message`/`handle_message_with_attachments`
  viraram wrappers finos de um `handle_turn` novo, mais geral, que aceita um `system_prompt:
  Option<&str>` opcional (a persona do agente) — dá `push` dela como a primeira mensagem quando
  não vazia, antes do contexto do vault. Zero mudança pros 3 canais existentes (CLI/Telegram/
  WhatsApp) e pro `DelegateTool`. `Orchestrator` também ganhou `with_model(model) -> Self`,
  barato (clona só bumps de `Arc`) — troca o modelo de uma chamada sem re-rodar `bootstrap()`
  inteiro (que reconectaria MCP servers, refaria OAuth, etc.)
- `crates/warden-bootstrap/src/lib.rs`: `AgentConfig { id, persona, provider_id }` novo,
  `FileConfig.agents: Vec<AgentConfig>`; `Conversation` ganhou `agent_id`/`provider_id:
  Option<String>` (`#[serde(default)]`, mesma retrocompatibilidade de `usage`/`attachments`) pra
  lembrar a última seleção de cada conversa; `build_model_provider` (antes `fn` privada) virou
  `pub` pro desktop poder construir um `ModelProvider` avulso a partir de um `provider_id`
- IPC `send_message` (`desktop/src-tauri/src/lib.rs`) ganhou `agent_id`/`provider_id:
  Option<String>` — só reconstrói o modelo quando `provider_id` de fato difere do provider ativo
  (evita trabalho à toa no caso comum de não trocar nada). `get_settings`/`save_settings` ganharam
  `agents: Vec<AgentPayload>`, com validação de nome vazio/duplicado (mesmo padrão de `providers`)
- Frontend: nova seção "Agents" na Settings (`AgentCard` — nome, `<textarea>` de personalidade,
  `<select>` de modelo padrão opcional — mirror de "Model providers"); nova barra `.chat-header`
  no topo do `ChatArea` com dois `<select>` (Agent/Model); `App.tsx` busca `get_settings` no mount
  e sempre que volta da tela de Settings, guarda a seleção corrente em estado, restaura a partir
  de `Conversation.agentId`/`providerId` ao trocar de conversa (caindo pro default se o id salvo
  não existir mais), e persiste a seleção atual a cada mensagem enviada
- `cargo build/test/clippy --workspace` limpos — 3 testes novos em `orchestrator/mod.rs`
  (`handle_turn_prepends_the_persona_as_the_first_message`, `handle_turn_ignores_a_blank_persona`,
  `with_model_swaps_the_model_used_without_touching_the_original`) usando os mesmos mocks já
  existentes no arquivo. `tsc`/`npm run build` limpos. Layout dos dois seletores na `.chat-header`
  e do `AgentCard` na Settings conferido via screenshot de harness estático reaproveitando o
  `App.css` de verdade, claro e escuro
- `PENDING.md`: P3 fechado; nova pendência **P32** registrada pro teste de ponta a ponta contra
  um provider real (mesma lacuna de P29/P30/P31 — sem chave de API no shell do agente)
- `PHASE.md` (Fase 1 e Fase 6) e `ARCHITECTURE.md` atualizados com as decisões desta sessão

**Próximo passo**: usuário confirmar de ponta a ponta na janela real (P32) — criar um agente,
conversar, trocar de modelo/agente no meio da conversa. Sem outro item de UX geral pendente além
do que já está em `PENDING.md`.

---

### 2026-09-03 — Sessão 42

- **Objetivo**: usuário pediu pra continuar o projeto; ofereci três frentes possíveis (App Mobile,
  expandir MCP, ou fechar o TTS de P28) e ele escolheu fechar o TTS — a última das três metades de
  P28 (Warden falar a resposta em voz).

**O que foi feito**:

- Duas decisões de produto confirmadas com o usuário antes de implementar: acionamento **manual**
  por botão em cada mensagem (não autoplay), e **reaproveitar a chave `whisper`** já existente em
  vez de pedir uma segunda API key (mesma conta OpenAI, dois endpoints de áudio)
- Novo módulo `crates/warden-core/src/speech.rs`, espelhando `transcribe.rs`: `synthesize_speech`
  faz `POST /v1/audio/speech` (`model: "tts-1"`, `voice: "alloy"`, fixos por enquanto) e devolve os
  bytes crus do mp3 (sem envelope JSON, ao contrário da transcrição)
- IPC novo `synthesize_speech` no desktop (`desktop/src-tauri/src/lib.rs`), mesma forma
  `AttachmentPayload` mime+base64 de sempre, mesma validação de chave ausente que `transcribe_audio`
  já tinha
- `MessageBubble.tsx` ganhou um botão de áudio (`SpeakerIcon`/`StopIcon` novos em `Icons.tsx`)
  junto da contagem de tokens, com três estados (idle/loading/playing) — toca via `<audio>` do
  próprio browser; clicar durante a reprodução para e reseta pro início (toggle tocar/parar, não
  play/pause)
- Novo util `desktop/src/lib/stripMarkdown.ts` (regex simples, sem dependência nova) — sanitiza o
  texto antes de mandar pro TTS, senão o `tts-1` lê a marcação markdown em voz alta
- `SettingsView.tsx`: label do campo existente renomeado pra "OpenAI voice API key
  (speech-to-text + text-to-speech)", deixando explícito que cobre as duas pontas agora — sem
  campo novo
- `cargo build/test/clippy --workspace` e `tsc`/`npm run build` limpos; sem teste unitário novo em
  `speech.rs` (resposta é bytes crus, não há wire-shape JSON pra testar). Layout do botão nos dois
  estados (idle e "tocando") conferido via screenshot de um harness estático reaproveitando o
  `App.css` de verdade, claro e escuro (mesmo método das Sessões 39-41, já que `invoke()` não
  existe fora do webview nativo do Tauri)
- `PENDING.md`: P28 fechado de vez (as três metades feitas), nova pendência **P31** registrada
  pro teste de ponta a ponta com uma chave de voz real (mesma lacuna que P29/P30)
- `PHASE.md` (Fase 6) e `ARCHITECTURE.md` atualizados com as decisões desta sessão

**Próximo passo**: nenhum item novo de UX geral pendente no Desktop além do que já está em
`PENDING.md` — os três testes de ponta a ponta de voz/imagem (P29/P30/P31) seguem bloqueados por
falta de chave de API real, sem prioridade imediata do usuário. Pelo `ROADMAP.md`, a próxima fase
grande na ordem combinada é **App Mobile** (Fase 7), já que Tools & MCP (Fase 5) está quase
completa.

---

### 2026-09-01 — Sessão 41

- **Objetivo**: Continuar de onde a Sessão 40 parou (commit feito, P29 registrada). Usuário
  escolheu puxar a metade "áudio" de P28 em seguida — escopada como só **input** de voz (TTS na
  resposta fica pra depois) via **transcrição universal** (Whisper, sempre, independente de qual
  dos 3 provedores de chat está ativo — não áudio nativo por provider, que deixaria Anthropic de
  fora).

**O que foi feito**:

- `ApiKeys.whisper: Option<String>` novo (`crates/warden-bootstrap`) — chave dedicada, mesmo
  padrão do `tavily`, independente do registry de providers de chat
- Novo módulo solto `crates/warden-core/src/transcribe.rs` (não é `ModelProvider` nem `Tool` —
  roda antes de `handle_message`, o modelo nunca invoca): `transcribe_audio(api_key, bytes,
  filename)` faz `POST multipart/form-data` pra `/v1/audio/transcriptions` da OpenAI
  (`model: "whisper-1"`), desserializa `{text}`. `reqwest` ganhou a feature `multipart`
  (workspace `Cargo.toml`)
- IPC do desktop: novo comando `transcribe_audio` (reaproveita `AttachmentPayload` da Sessão 40 —
  mesma forma mime+base64), lê a chave Whisper do config, erro claro se não configurada.
  `SettingsSnapshot`/`SettingsFormPayload` ganharam `whisper_key`
- Settings: novo `ApiKeyField` "Whisper API key (voice input)" logo abaixo do Tavily
- Composer: botão de microfone novo (`MicIcon`) ao lado do de anexo. Texto transcrito cai no
  campo de mensagem pro usuário revisar antes de mandar (não envia sozinho). Estado "gravando"
  com ícone vermelho pulsante (`.chat-mic-btn--recording`, nova animação CSS)
- Testes: 1 unitário novo (`transcribe.rs`, só desserialização da resposta `{"text": "..."}") —
  sem infra de mock HTTP no projeto, mesmo padrão da Sessão 40 (P29)
- **Virada no meio da sessão**: a primeira implementação da gravação usava `MediaRecorder`/
  `getUserMedia` do browser. O usuário testou na janela real e bateu direto no problema
  registrado como risco em `PENDING.md` P30 — WebKitGTK nega `getUserMedia` por padrão no Linux
  (`NotAllowedError`), porque o Tauri/wry nunca conecta o sinal `permission-request` que o WebKit
  exige pra sequer perguntar (limitação conhecida, sem solução oficial do próprio time do Tauri —
  issues [#12547](https://github.com/tauri-apps/tauri/issues/12547) e
  [#8346](https://github.com/tauri-apps/tauri/issues/8346)). Perguntado ao usuário: hack
  GTK-específico (conectar o sinal na mão, só resolve no Linux, abaixo do padrão de segurança do
  próprio Tauri) vs captura nativa via `cpal` (cross-platform, já era o plano B documentado).
  Usuário escolheu `cpal`. Implementado: novo módulo `desktop/src-tauri/src/recording.rs` — grava
  numa thread OS dedicada (o `cpal::Stream` não é `Send` de forma confiável entre plataformas,
  então nunca sai da thread que o criou), bloqueando num canal até `stop()` sinalizar; downmix de
  qualquer formato (F32/I16/U16, qualquer nº de canais) pra mono `i16`; WAV via `hound`, devolvido
  como o mesmo `AttachmentPayload` que a imagem já usa. Novos comandos IPC
  `start_recording`/`stop_recording` substituem o `MediaRecorder` no frontend —
  `transcribe_audio` não mudou nada (só passou a receber `audio/wav`)
- **Imprevisto à parte**: o workspace ficou sem espaço em disco no meio da verificação
  (`/home` 100% cheio, `target/` em 33G) — mesmo tipo de problema já visto na Sessão 39.
  `cargo clean` (liberou os 33G) + rebuild do zero (~5min) resolveu; não é um problema do código
- Verificação: `cargo build/test/clippy --workspace` limpos (`warden-core` com 25 testes,
  `warden-bootstrap` com 25, ambos após o rebuild do zero); `npx tsc --noEmit`/`npm run build`
  limpos (rodados de novo depois da virada pro `cpal`); layout do botão de microfone (parado e
  "gravando") conferido via screenshot Playwright, mesmo método das sessões anteriores.
  **Não testado ainda**: chamada real ao Whisper (sem chave neste shell) nem a captura de áudio
  de ponta a ponta com a implementação `cpal` nova — registrado como `PENDING.md` P30 (atualizada
  pra refletir a virada)
- `project/PENDING.md` (P28 fechada pra input, reaberta só pra TTS output; P30 nova, depois
  atualizada pra refletir a virada pro `cpal`), `project/ARCHITECTURE.md` (7 decisões, 1 revertida
  no meio da sessão)

**Próximo passo**: P30 (testar voz de ponta a ponta na janela real, agora com a captura nativa —
colar uma chave Whisper em Settings, clicar no microfone, falar, checar a transcrição) e/ou P29
(mesmo teste pendente pro anexo de imagem, ainda não feito). **Atualizado ainda na mesma
sessão**: usuário avisou que uma chave da OpenAI não vai rolar tão cedo — os dois testes (P29,
P30) ficam registrados como pendência de baixa prioridade, adiados sem data definida em vez de
bloquear o trabalho. Se a chave da OpenAI não entrar nos planos a médio prazo, vale reconsiderar
o Whisper como STT de P30 (ver nota de transcrição local em `PENDING.md`).

---

### 2026-09-01 — Sessão 40

- **Objetivo**: Retomar de onde a Sessão 39 parou. Usuário escolheu puxar P28 — anexo de
  imagem no chat do desktop (metade de "imagem", sem áudio; metade "imagem" sem PDF/docs
  genéricos, ambos escopo explícito do usuário).

**O que foi feito**:

- Multimodal de verdade em `warden-core`: `Attachment{mime_type, data}` novo, `Message` ganhou
  `attachments: Vec<Attachment>` + construtor `user_with_attachments`. Os três providers
  (`OpenAiProvider`, `AnthropicProvider`, `GeminiProvider`) passaram a codificar isso no formato
  multimodal nativo de cada API — `image_url`/data-URI, bloco `image`/`base64`, `inlineData`,
  respectivamente. `Orchestrator` ganhou `handle_message_with_attachments` (o `handle_message`
  existente virou um wrapper fino chamando ele com `Vec::new()`) — CLI/Telegram/WhatsApp/
  `DelegateTool` continuam exatamente como estavam, só o `send_message` do desktop usa o novo
  método
- IPC do desktop: `ChatTurn`/`send_message` ganharam `attachments`; novo comando
  `read_attachment(path)` lê o arquivo escolhido no diálogo nativo (`@tauri-apps/plugin-dialog`,
  já usado pro vault path), valida extensão (png/jpg/jpeg/webp/gif, rejeita o resto com
  "Unsupported file type") e devolve base64 já pronto. `base64` virou dependência declarada
  direto (`workspace.dependencies`, já era transitiva)
- Persistência: `ConversationMessage.attachments` (`#[serde(default)]`, mesma retrocompatibilidade
  do `usage` opcional) — a imagem sobrevive ao reload da conversa e volta pro modelo em qualquer
  volta seguinte, não só na primeira mensagem
- Frontend: `MessageInput.tsx` ganhou botão de anexo (`AttachIcon`/`CloseIcon` novos em
  `Icons.tsx`) com preview de thumbnails removíveis acima do composer; `MessageBubble.tsx` mostra
  a imagem na bolha do usuário; `App.tsx`/`ChatArea.tsx`/`types.ts` fiados ponta a ponta
- Testes unitários novos nos três providers (`to_chat_message`/`to_anthropic_message`/
  `to_content`), verificando via `serde_json::to_value` que o JSON de saída bate com o formato
  esperado por cada API — nenhum provider tinha teste HTTP-mockado até agora (sem infra de mock
  nesse nível no projeto), então ficou nesse nível em vez de introduzir uma
- Verificação: `cargo build/test/clippy --workspace` limpos (6 testes novos, todos os outros
  intactos); `npx tsc --noEmit`/`npm run build` limpos; layout do botão de anexo, preview de
  thumbnail e bolha com imagem conferido via screenshot Playwright headless (claro e escuro,
  mesmo método da Sessão 39 — dev server real pro composer vazio, HTML estático reaproveitando o
  `App.css` de verdade pro preview/bolha com conteúdo). `npm run tauri dev` deixado rodando pro
  usuário testar de ponta a ponta contra um provider real (nenhuma chave de API disponível neste
  shell)
- `project/PENDING.md` (P28 dividida: metade imagem resolvida, só áudio segue em aberto; **P29
  nova** registrando o teste de ponta a ponta contra um provider real ainda pendente),
  `project/ARCHITECTURE.md` (4 decisões novas)
- Decisão do usuário nesta sessão: commitar o que já foi feito, registrar o teste de ponta a
  ponta como pendência (P29) em vez de bloquear nele, e seguir com o resto da implementação
  (áudio/PDF) numa sessão futura

**Próximo passo**: P29 (testar anexo de imagem de ponta a ponta contra um provider real) e/ou
seguir pra outra frente de P28 (áudio) ou outra pendência do backlog — a definir quando a
próxima sessão retomar.

---

### 2026-08-31 — Sessão 39

- **Objetivo**: Usuário pediu pra subir o app desktop e depois reformular a UI — "não tá com cara
  de IA, parece mais um chat basicão", pedindo puxar mais pro estilo ChatGPT. Sinalizou também
  (pra depois, não nesta sessão) anexo de arquivo/imagem e envio de áudio.

**O que foi feito**:

- `npm run tauri dev` rodado em background — janela nativa de verdade aberta na tela do usuário
  (precisou de `cargo clean` numa sessão anterior por espaço em disco, então essa foi a primeira
  compilação do zero; ~3min)
- Restyle completo do chat, mantendo a identidade roxa e o sistema `--color-*` light/dark já
  existentes (ver `ARCHITECTURE.md` pro detalhamento): mensagens do assistente sem caixa (texto
  solto + avatar circular novo, a marca "escudo" do Warden), coluna de conversa centralizada,
  composer virou pílula flutuante com botão de enviar circular, indicador de "pensando" novo
  (não existia nenhum feedback de carregamento antes), sidebar com marca no topo e Settings
  movido pro rodapé, empty-state virou hero centralizado com logo + saudação. Ícones SVG à mão
  novos em `components/Icons.tsx` (`LogoMark`, `PlusIcon`, `SettingsIcon`, `SendIcon`) — sem
  adicionar uma lib de ícones nova. Título da janela (`index.html`) trocado do boilerplate padrão
  do template Tauri+React+Vite pra "Warden" (nunca tinha sido tocado)
- Verificado via screenshot Playwright headless contra o próprio dev server do Tauri
  (`localhost:1420`, mesma URL que a janela nativa carrega) — layout/CSS em claro e escuro; e um
  HTML estático à parte reaproveitando o `App.css` de verdade pra validar bolha/avatar/markdown/
  indicador de "pensando" com conteúdo real, já que `invoke()`/IPC não funciona fora do webview
  nativo (headless Chromium não tem isso). A janela nativa que o usuário já tinha aberta atualiza
  sozinha via Vite HMR — fica pra ele confirmar visualmente o resultado final ali. `npx tsc
  --noEmit`/`npm run build` limpos
- Escopo deliberadamente **não** incluído (usuário pediu "depois"): anexo de arquivo/imagem e
  envio de áudio — registrado como pendência nova `P28` em `PENDING.md` (não implementado, exige
  suporte multimodal em `ModelProvider`/`Message` do `warden-core`, já uma lacuna maior ligada a
  P21). Nenhum botão de anexo "morto" foi adicionado no composer só de placeholder
- `project/ARCHITECTURE.md` (2 decisões novas), `project/PENDING.md` (P28 nova)

**Próximo passo**: usuário confirma visualmente na janela aberta se o resultado bate com o que ele
tinha em mente; se sim, pode fazer sentido puxar P28 (anexos/áudio) em seguida, já que foi o
próximo item que ele mesmo sinalizou.

---

### 2026-08-31 — Sessão 38

- **Objetivo**: Usuário pediu integração com Discord, sem urgência ("acho que vai ser útil no
  futuro"). Perguntei o tipo (bot vs MCP server) e se era pra implementar já ou só registrar —
  usuário escolheu **as duas frentes, só documentar por enquanto**, nada de código.

**O que foi feito**:

- Registrada a visão em `ROADMAP.md` (nova seção "Integração com Discord") e pendência nova
  `P27` em `PENDING.md`, cobrindo as duas frentes: Warden como bot no Discord (canal novo, mesmo
  espírito do Telegram/WhatsApp, mas protocolo de gateway/WebSocket próprio da Discord, diferente
  dos dois padrões já existentes) e Discord como MCP server (mesmo mecanismo de preset já usado
  pro Slack/Notion/GitHub — mais rápido de ligar, sem código novo)

**Próximo passo**: usuário não indicou quando retomar — quando isso voltar à tona, vale perguntar
qual das duas frentes puxar primeiro (o MCP é o caminho mais rápido).

---

### 2026-08-31 — Sessão 37

- **Objetivo**: Usuário voltou pedindo pra continuar; ofereci 3 opções (mais presets MCP, Fase
  7/Mobile, fechar P26 — OAuth pro transporte HTTP do MCP) e ele escolheu **P26**.

**O que foi feito**:

- Investigação prévia (antes de planejar): `rmcp` v3.1.2+ (a versão travada no `Cargo.lock` era
  3.1.0) já embute um client OAuth completo atrás da feature `auth` — discovery RFC 9728/8414,
  Dynamic Client Registration, PKCE, renovação automática de token, e um adapter `AuthClient<C>`
  que pluga direto no transporte streamable-HTTP já usado desde a Sessão 36. Isso mudou o escopo
  da tarefa de "implementar OAuth" pra "encaixar o que o `rmcp` já resolve" — plano desenhado e
  aprovado em `/home/masterlxz/.claude/plans/buzzing-churning-sun.md` antes de mexer em código
- `cargo update -p rmcp` (3.1.0 → 3.1.4, sem tocar `Cargo.toml`, que já pedia só `"3"`); feature
  `auth` adicionada em `crates/warden-core/Cargo.toml`, junto com uma segunda dependência
  `reqwest` aliasada (`reqwest-oauth = { package = "reqwest", version = "0.13.2" }`) — só pra
  poder nomear o tipo `reqwest::Client` que `AuthClient::new` exige, já que o `rmcp` não reexporta
  esse tipo em nenhum caminho público (Cargo unifica com a cópia que o `rmcp` já trazia, não é uma
  terceira versão no grafo). `tokio` do workspace ganhou as features `net`/`io-util` (pro listener
  local do callback)
- Novo módulo `crates/warden-core/src/tool/mcp_oauth.rs`: `connect_http_oauth` (caminho headless,
  usado em todo `bootstrap()` — falha com mensagem clara e acionável se não houver token guardado,
  tratado como qualquer outra falha de conexão MCP, não fatal) e `authorize_interactively`
  (caminho interativo — abre o browser via uma closure que o chamador fornece, escuta um redirect
  local numa porta TCP efêmera implementado à mão sobre `tokio::net::TcpListener` em vez de
  promover `axum` a dependência de produção só pra uma request fire-and-forget, troca o código por
  token, persiste). `FileCredentialStore` novo (mesmo módulo) implementa a trait `CredentialStore`
  do `rmcp` sobre um arquivo JSON por server (`~/.config/warden/mcp_oauth/<nome-sanitizado>.json`,
  texto puro — mesma postura de segurança que toda outra credencial do Warden hoje). `McpToolProvider`
  ganhou um construtor `pub(crate) fn from_session` pra esse módulo poder produzir o mesmo tipo que
  `connect_stdio`/`connect_http` já produzem sem duplicar `tools()`/`call()`
- `crates/warden-bootstrap/src/lib.rs`: `McpServerConfig::Http` ganhou um campo `oauth: bool`
  (`#[serde(default)]` — todo `config.toml` escrito desde a Sessão 36 continua parseando sem
  mudança), novo helper `oauth_credential_store_path(name)`, e o loop de conexão do `bootstrap()`
  passou a rotear pro `connect_http_oauth` quando `oauth` é `true`
- Desktop: `McpServerHttp` (`types.ts`) ganhou `oauth: boolean`; `SettingsView.tsx` — checkbox
  "Requires OAuth" no card HTTP (esconde os headers, mostra um painel de status/Connect/Disconnect
  novo, `McpOAuthPanel`); preset do Slack trocou "cole seu bearer token" por `oauth: true`. Três
  comandos Tauri novos em `desktop/src-tauri/src/lib.rs`: `mcp_oauth_status` (sem chamada de rede —
  só confere se o arquivo de credencial existe), `mcp_oauth_connect` (roda o fluxo interativo,
  abre o browser via `tauri_plugin_opener::open_url`, já existente desde antes pra links do chat),
  `mcp_oauth_disconnect` (apaga a credencial)
- **Verificado de ponta a ponta contra um server OAuth real, não mockado** (mesmo rigor das
  Sessões 35/36): `crates/warden-core/tests/mcp_oauth.rs` novo — um único server `axum` de teste
  faz o papel de protected resource *e* de authorization server ao mesmo tempo (mesma forma
  auto-referente que o próprio `tests/test_client_credentials.rs` do `rmcp` usa pro grant mais
  simples), com discovery, Dynamic Client Registration, `/authorize` e `/token` reais. O único
  passo necessariamente falsificado é o `/authorize` aprovar sozinho em vez de mostrar uma tela de
  consentimento pra um humano de verdade (nada automatizado clica "Allow") — tudo rio abaixo
  disso roda de verdade: a troca PKCE, a persistência em arquivo, e a reconexão headless usando só
  o token salvo. O `open_browser` do teste faz uma requisição HTTP real contra a URL de
  autorização (em vez de abrir um browser de verdade), que segue o redirect do servidor de teste e
  bate direto no listener local do próprio módulo — provando que o listener funciona de verdade,
  não só o protocolo OAuth em volta dele. Passou de primeira. `cargo build/test/clippy --workspace
  --all-targets` limpos (precisou de um `cargo clean` no meio do caminho — o `target/` sozinho
  tinha crescido pra 33G e encheu a partição `/home`, 0 bytes livres; nada a ver com o código,
  só acúmulo de builds anteriores); `npx tsc --noEmit`/`npm run build` limpos no frontend
- `project/ARCHITECTURE.md` (6 decisões novas), `project/PHASE.md` (nota P26 resolvida na Fase 5),
  `project/PENDING.md` (P26 resolvida)

**Não verificado**: conexão contra o Slack real (`https://mcp.slack.com/mcp`) — exigiria um
workspace Slack de verdade e um humano clicando "Allow" no browser, fora do alcance de uma sessão
automatizada. Se o Slack não suportar Dynamic Client Registration na prática (só descobrível
testando), falta expor um caminho de `client_id` pré-cadastrado na UI — `rmcp` já suporta isso
(`AuthorizationRequest::with_preregistered_client`), só não foi conectado a nada ainda. Também sem
teste de UI interativo (mesma limitação de Wayland/`xdotool` já registrada em sessões anteriores).

**Próximo passo**: usuário não indicou ainda — pedir pra ele testar "Connect" contra o Slack real
seria o próximo passo natural pra fechar de vez esse fio solto; fora isso, as opções que ficaram de
fora desta escolha (mais presets MCP, Fase 7/Mobile) continuam válidas.

---

### 2026-08-31 — Sessão 36

- **Objetivo**: Usuário voltou pedindo pra continuar o roadmap; ofereci 3 opções (P25 — transporte
  HTTP no client MCP; mais presets MCP; avançar pra Fase 7/Mobile) e ele escolheu **P25**.

**O que foi feito**:

- `crates/warden-core/src/tool/mcp.rs`: `McpToolProvider::connect_http(name, url, headers)` novo,
  ao lado do `connect_stdio` já existente — mesma interface `ToolProvider`/`Tool` depois de
  conectado. Implementado com `StreamableHttpClientTransport::from_config(...)` do `rmcp`
  (feature `transport-streamable-http-client-reqwest` + `reqwest`), headers customizados via
  `HashMap<HeaderName, HeaderValue>` (nova dependência direta em `http = "1"`, workspace-level)
- Descoberto no processo: o `rmcp` v3.1 depende da sua **própria** cópia de `reqwest 0.13`
  (major diferente do `reqwest 0.12` que o resto do Warden usa), e a única feature `rustls` desse
  `reqwest 0.13` liga `aws-lc-rs` (C compilado via `cmake`) em vez do `ring` puro-Rust que o
  `reqwest 0.12` ainda usa — trade-off aceito (registrado em `ARCHITECTURE.md`) em vez de
  reimplementar o transporte HTTP na mão só pra evitar isso
- `crates/warden-bootstrap/src/lib.rs`: `McpServerConfig` virou um enum `#[serde(untagged)]`
  (`Stdio{name,command,args,env}` / `Http{name,url,headers}`) — discriminado pela presença de
  `command` vs `url`, sem precisar de uma tag `transport` nova, então todo `config.toml` escrito
  desde a 5.2 continua parseando sem tocar em nada. `register_mcp_tools` (renomeado de
  `register_mcp_server_tools`) agora recebe o `Result<McpToolProvider, _>` já pronto em vez de
  parâmetros de conexão, compartilhado pelos dois transportes e pelo Tavily
- `desktop/src-tauri/src/lib.rs`: `save_settings` valida cada `McpServerConfig` por variante
  (nome+comando pra Stdio, nome+URL pra Http). `desktop/src/types.ts` ganhou `McpServerStdio`/
  `McpServerHttp`/`isMcpServerHttp` (união discriminada pela mesma presença de campo do lado
  Rust, sem tag sintética). `SettingsView.tsx`: `McpServerCard` ganhou um select "Transport" que
  troca a forma do objeto; `KeyValueListField` novo extrai a lista chave/valor repetida (antes só
  em env vars, agora também em headers). Novo preset "Slack (hosted — needs a bearer token)" com
  a URL confirmada via pesquisa (`https://mcp.slack.com/mcp`) e aviso de que o endpoint real exige
  OAuth completo, que este client não implementa (só headers estáticos) — P25 fica **parcialmente**
  resolvida, registrada nova pendência P26 pra isso
- **Verificado de ponta a ponta com um server real, não mockado** (mesmo rigor da Sessão 35):
  `crates/warden-core/tests/mcp_http.rs` novo — um server MCP de verdade (`axum` + `rmcp`
  server-side) bindado numa porta local de verdade, com middleware de auth que exige um header
  específico. Dois testes: conecta/lista/chama uma tool de ponta a ponta com o header certo; e
  confirma que a conexão **falha** sem o header — prova de que os headers customizados realmente
  chegam no server, não só são aceitos e descartados client-side. Dev-dependencies novas:
  `axum = "0.8"`, `tokio-util = "0.7"`, features `server`/`macros`/`transport-streamable-http-server`
  do `rmcp` (todas dev-only, o binário shipado não ganha peso)
- `cargo build/test/clippy --workspace --all-targets` limpos (2 testes novos em `warden-core`, 2 em
  `warden-bootstrap` — `parses_http_mcp_server_from_toml`/round-trip do `save_config`, resto sem
  quebra); `npx tsc --noEmit`/`npm run build` limpos no frontend
- `project/ARCHITECTURE.md` (5 decisões novas: API HTTP em si, trade-off de dependência
  `aws-lc-rs`, formato de config, modelo de autenticação, UI do desktop), `project/PHASE.md`
  (nota P25 resolvida na Fase 5), `project/PENDING.md` (P25 resolvida parcialmente, P26 nova)

**Próximo passo**: usuário não indicou ainda — as opções que ficaram de fora desta escolha
(mais presets MCP, Fase 7/Mobile) continuam válidas, junto com o resto do `ROADMAP.md`.

---

### 2026-08-29 — Sessão 35

- **Objetivo**: Usuário voltou depois de duas semanas com uma re-priorização grande do roadmap
  ("bora fazer isso aqui ser o melhor agente pessoal possível") — nova ordem: Desktop (UX,
  gerenciamento de API keys, múltiplos provedores) → MCP → Mobile → CLI/servidor/vault(Arweave) →
  TruthID. Escopo desta sessão, confirmado explicitamente pelo usuário: focar só no primeiro item,
  registrar o resto como pendência.

**O que foi feito**:

- Re-priorização registrada em `ROADMAP.md` (nova ordem de sequenciamento, 2026-08-29) e novas
  pendências em `PENDING.md`: P22 (múltiplos provedores), P23 (gerenciamento de API keys),
  P24 (Fase 4 precisa repensar IPFS→Arweave — confirmado nesta sessão que o **TruthID** já migrou
  pra Arweave, via leitura de `~/Documents/workspace/truthid/docs/docs/sdk/dart.md`: carteira por
  identidade, ponteiro `ar://` em vez de CID IPFS), e P8 atualizada (usuário reafirma que a UX do
  CLI "tava feio pra krl" mesmo após o polish das Sessões 31/33 — fica pra depois do Desktop/MCP)
- Duas perguntas de escopo feitas antes de implementar (trade-off real, não óbvio): (1) arquitetura
  de múltiplos provedores — enum fechado com mais braços (`Provider::Anthropic`/`Provider::Ollama`)
  vs um **registry** de entradas configuráveis → usuário escolheu **registry**, no espírito do
  `[[mcp_servers]]` já existente; (2) quais provedores entram — Anthropic+Ollama dedicados vs
  Anthropic dedicado + um tipo genérico "OpenAI-compatível" (cobre Ollama e qualquer outro server
  que fale o mesmo protocolo, tipo Groq/OpenRouter/DeepSeek) → usuário escolheu a **genérica**
- Implementado (`warden-core`): novo `AnthropicProvider` (`model/anthropic.rs`) — Messages API
  própria (system como campo top-level, `tool_use`/`tool_result` como content blocks, `max_tokens`
  fixo em 4096 sem config própria ainda); `OpenAiProvider` ganhou `base_url` configurável
  (`with_base_url`, default continua a OpenAI oficial) — cobre Ollama/OpenRouter/Groq/etc. de
  graça, sem implementação dedicada por empresa
- Implementado (`warden-bootstrap`): `Provider` ganhou `Anthropic`/`OpenaiCompatible`;
  `ProviderConfig { id, kind, api_key, base_url, model }` novo; `FileConfig` ganhou
  `providers: Vec<ProviderConfig>` + `active_provider: Option<String>`. Nova
  `resolve_model_provider`: usa o registry quando não-vazio (erro claro se `active_provider` não
  bater com nenhum `id`), senão sintetiza uma entrada a partir dos campos antigos
  (`provider`/`api_keys.gemini`/`api_keys.openai` + env vars `GEMINI_API_KEY`/`OPENAI_API_KEY`) —
  os campos antigos continuam no struct só como fallback (deprecated, `deny_unknown_fields`
  obrigava manter em vez de quebrar `config.toml` já existentes, inclusive o real do usuário desde
  a Sessão 32). `default_model_for` virou `Option<&str>` (`None` pra `OpenaiCompatible`, sem
  default universal). 9 testes novos cobrindo registry/fallback/erros claros
- Desktop: `SettingsSnapshot`/`SettingsFormPayload` (`src-tauri/src/lib.rs`) reescritos pro shape
  de lista (`providers: Vec<ProviderPayload>` + `active_provider`), com validação de nome
  vazio/duplicado no backend; `get_settings` expõe `default_models` (por kind) pro placeholder do
  campo Model. `SettingsView.tsx` ganhou uma seção "Model providers": cards com nome/tipo (select)/
  API key (mascarada, reveal toggle)/Base URL (só pra `openai_compatible`)/Model, rádio "Active",
  botão apagar, "+ Add provider" — tudo editado localmente e salvo de uma vez, mesmo padrão do
  resto da tela (não introduziu CRUD granular via IPC)
- Ajustes mecânicos em cascata: `Overrides` ganhou `provider_id` (reservado pra quando um canal
  quiser selecionar por id de registry, nenhum ainda usa); `warden-cli`/`warden-telegram`/
  `warden-whatsapp` (`main.rs`) só precisaram de `..Default::default()` no literal de `Overrides`
- Verificado: `cargo build/test/clippy --workspace --all-targets` limpos (23 testes novos/
  atualizados só no `warden-bootstrap`, resto sem quebra — nenhum teste de CLI/Telegram/WhatsApp
  precisou mudar, confirmando que o fallback preservou o comportamento de zero-config via env var
  exatamente como antes), `npx tsc --noEmit`/`npm run build` limpos no frontend. **UI testada de
  ponta a ponta de verdade** via Playwright headless contra o dev server real (mesmo padrão da
  Sessão 22, instalado e removido só pro teste): adicionar 2 provedores, trocar tipo pra
  `openai_compatible` (Base URL aparece dinamicamente), marcar um como ativo, salvar — payload
  conferido byte a byte no formato exato que o Rust espera (`kind: "openai_compatible"`,
  `active_provider` certo), zero erros de console, apagar o provedor não-ativo preserva o ativo
  corretamente. Screenshot conferida visualmente (tema roxo consistente com o resto do app)
- `project/ARCHITECTURE.md` (4 decisões novas: arquitetura registry, quais provedores, compat
  com config antigo, UI do desktop), `project/PHASE.md` (nota de polish na Fase 6),
  `project/OVERVIEW.md` (status Fase 5/6 corrigido — Fase 5 estava marcada "Pendente" mas só falta
  a 5.4, bloqueada pela Fase 8), `project/PENDING.md` (P22/P23 resolvidas, P24 nova, P8 atualizada)

**Continuação da mesma sessão — Fase 5 (Tools & MCP)**: usuário escolheu (múltipla escolha, as
3 opções oferecidas) atacar de uma vez a UI de gerenciamento de servers MCP, pré-configurar mais
integrações populares, e Warden como server MCP — os dois lados de P11 em `PENDING.md`.

- Pesquisa real (WebSearch) antes de decidir presets, mesmo rigor das integrações Google/Tavily/
  filesystem: **Notion** limpo (`@notionhq/notion-mcp-server`, oficial, `npx`, ativo — entra);
  **GitHub** — pacote npm oficial (`@modelcontextprotocol/server-github`) está **arquivado**
  (mesmo destino do Google Drive na Sessão 26), substituto ativo roda via **Docker**, não `npx`;
  **Slack** — pacote npm oficial descontinuado, substituto é hospedado remotamente pela própria
  Slack (HTTP/OAuth, fora do alcance do client MCP hoje, só stdio). Pergunta feita ao usuário:
  aceitar Docker como segundo runtime opcional (só quem quiser GitHub) vs só Notion agora →
  escolhido **Notion + GitHub via Docker**; Slack ficou de fora (P25 nova em `PENDING.md`)
- Implementado — **P11a (client MCP, UI)**: nova seção "MCP servers" na tela de Settings
  (`SettingsView.tsx`), cards com nome/comando/argumentos (textarea, um por linha)/variáveis de
  ambiente (linhas dinâmicas chave/valor), 4 botões de "quick add" (Filesystem/Google
  Workspace/Notion/GitHub via Docker) além de "+ Custom". Backend (`desktop/src-tauri/src/
  lib.rs`): `McpServerConfig` reusado direto como tipo de IPC (campos já de uma palavra só, sem
  remapeamento), validação de nome/comando vazio, `save_settings` passou a usar o payload como
  fonte de verdade (parou de só carregar-e-devolver o `mcp_servers` existente)
- Implementado — **P11b (Warden como server MCP)**: `Orchestrator` ganhou `tools() -> &[Arc<dyn
  Tool>]` (getter novo, mesmo espírito do `vault()` já existente); novo crate/binário
  `crates/warden-mcp-server` — chama `bootstrap()` normal (mesmo conjunto de tools que qualquer
  canal teria) e re-expõe via `ServerHandler` do próprio `rmcp` (mesma API server-side já provada
  em `warden-core/tests/mcp_stdio.rs`, agora em produção pela primeira vez — `rmcp` ganhou
  `server`+`transport-io` como dependência real). `list_tools` traduz `ToolSpec`→formato MCP,
  `call_tool` despacha pro `Tool::call` já existente
- Verificado de ponta a ponta de verdade, não mockado: **client MCP real conectando no
  `warden-mcp-server` real** via subprocesso (`crates/warden-mcp-server/tests/mcp_server.rs`,
  2 testes novos — lista as 3 tools reais e faz um round-trip `write_file`→`read_file` de
  verdade; e confirma que falha claro sem API key configurada), mesmo cuidado de isolamento de
  `HOME` da Sessão 32 (senão o teste vazaria pro config real da máquina). UI de MCP servers
  testada via Playwright headless (mesmo padrão usado pros provedores mais cedo nesta sessão):
  quick-add Notion + servidor custom, edição de env vars, payload conferido byte a byte, zero
  erros de console, screenshot conferida visualmente
- `cargo build/test/clippy --workspace --all-targets` limpos (2 testes novos no
  `warden-mcp-server`, resto sem quebra), `npx tsc --noEmit` limpo
- `project/ARCHITECTURE.md` (5 decisões novas: UI de servers MCP, presets pesquisados, GitHub via
  Docker, Warden-como-server e seu transporte), `project/PHASE.md` (nota em Fase 5: P11
  resolvida), `project/PENDING.md` (P11 resolvida nas duas direções, P25 nova — client MCP só
  stdio, bloqueou Slack)

**Próximo passo**: Próximo item da nova ordem do roadmap (ver `ROADMAP.md`) é **App Mobile**
(Fase 7). Depois disso, CLI/app servidor/Vault(Arweave)/TruthID, nessa ordem. Pendência do
próprio usuário, ainda sem confirmação: revogar a API key Gemini exposta em texto puro na
Sessão 32.

---

### 2026-08-15 — Sessão 34

- **Objetivo**: Confirmar de vez a UX interativa corrigida na Sessão 33 (o usuário ainda não tinha
  testado depois do fix de cores) — nada mudou no repo entre 2026-08-09 e hoje.

**O que foi feito**:

- Rodado `warden` de verdade via pty real (Python `pty` + `select`, já que o Bash tool não aloca
  TTY) com a pergunta "liste 3 frutas em markdown", capturando o stream bruto de bytes/ANSI de
  ponta a ponta
- Confirmado visualmente no dump bruto: banner "Warden" verde negrito, prompt `>` ciano negrito,
  spinner braille com "Thinking..." esmaecido animando entre frames, label de resposta "● Warden"
  verde negrito, bullets da lista em ciano (`\x1b[38;5;14m`), linha de contagem de tokens
  esmaecida — todos os elementos da Sessão 33 presentes e funcionando juntos num fluxo real
  (pergunta → spinner → resposta formatada → `exit` limpo)
- Nenhuma mudança de código necessária — sessão de confirmação/QA, não de implementação

**Próximo passo**: Pendência que segue em aberto e é só do usuário (fora do meu alcance):
revogar/gerar nova a API key Gemini que foi colada em texto puro no chat da Sessão 32. Depois
disso, ou já decidir seguir para a Fase 4 (Vault & Memória — espelho IPFS, cifra, versionamento,
busca semântica), que é a próxima fase não iniciada do roadmap.

---

### 2026-08-09 — Sessão 33

- **Objetivo**: Usuário testou a UX rica do terminal (Sessão 31/32) de verdade e reportou "ainda
  tá horrivel, não mudou nada o visual" — bug real, investigar e corrigir.

**O que foi feito**:

- Investigado com um pseudo-terminal real (`pty` do Python, já que o Bash tool não aloca TTY, e
  `Stdio::piped()` num teste não conta como TTY) — confirmou que o caminho interativo (rustyline,
  histórico, spinner) já estava disparando corretamente, então não era um bug de wiring/binário
  desatualizado
- Usuário confirmou o sintoma exato: "tipo essas coisas até funcionam, mas visualmente só tem o
  sinal de maior" — histórico e spinner funcionam, mas nenhuma cor aparece
- **Causa raiz**: `MadSkin::default()` do `termimad` só estiliza sintaxe markdown (negrito,
  itálico, headers) — texto corrido puro, que é a maior parte de uma resposta de LLM, sai sem
  nenhuma cor. E nenhum elemento da "moldura" (prompt `> `, banner, label da resposta, spinner)
  tinha sido colorido — só a linha de tokens usava `owo-colors` (Sessão 31)
- Corrigido em `crates/warden-cli/src/interactive.rs`:
  - `response_skin()` novo: customiza o `MadSkin` com cores explícitas via
    `termimad::crossterm::style::Color` (negrito amarelo, itálico magenta, headers e bullets
    ciano, código inline verde)
  - Banner de abertura ganhou "Warden" em verde negrito + subtítulo esmaecido
  - Prompt colorido: `>` ciano negrito (testado — `rustyline` aceita ANSI no prompt sem quebrar
    a edição de linha)
  - Cada resposta ganhou um label "● Warden" em verde negrito antes do texto renderizado (mesmo
    espírito visual do bullet do Claude Code)
  - Spinner "Thinking..." agora esmaecido em vez de texto puro
- Reinstalado (`cargo install --path crates/warden-cli --force`) e verificado de ponta a ponta
  com uma pergunta real (`liste 3 frutas em markdown`) via pty real — confirmado visualmente: cores
  aplicadas em todos os elementos, bullets da lista em ciano, resposta rendendo formatada

**Próximo passo**: Usuário testar de novo no terminal dele e confirmar se a UX agora está no nível
esperado (estilo Claude Code/Codex/Kimi Code). Revogar a API key exposta no chat (pendência da
Sessão 32, ainda não confirmada como feita).

---

### 2026-08-09 — Sessão 32

- **Objetivo**: Dois bugs reais encontrados pelo usuário rodando o `warden` de verdade depois da
  Sessão 31 (UX do terminal).

**O que foi feito**:

- **Bug 1 — modelo Gemini default desatualizado**: usuário criou `~/.config/warden/config.toml`
  de verdade com uma chave real e rodou `warden` — erro 404 da API, `gemini-2.5-flash` "no longer
  available to new users". Isso já tinha sido sinalizado como risco na Sessão 1 (`SESSIONS.md`:
  "vale confirmar em aistudio.google.com se ainda é o correto"). Corrigido `default_model_for`
  (`crates/warden-bootstrap/src/lib.rs`) pra `gemini-3.5-flash` (geração atual confirmada via
  busca — Gemini 3.x é a linha viva em 2026-08, 2.5 vira fallback pago sendo desligado em
  outubro); comentário novo no código explicando que esses defaults ficam velhos e por quê
- **Nota de segurança**: usuário colou a própria API key real em texto puro no chat — fica salva
  no histórico da conversa. Recomendado revogar/gerar uma nova em aistudio.google.com
- **Bug 2 — bug real de isolamento de teste, achado ao rodar a suíte completa depois da correção
  do modelo**: `fails_clearly_without_a_gemini_key` (`warden-cli/tests/cli.rs`) começou a **passar
  quando devia falhar** — `env_clear()` sozinho não isola `dirs::config_dir()` do `HOME` real: com
  `HOME` ausente, o crate `home` cai pra uma busca via libc/getpwuid do diretório real do usuário,
  então o teste acabou lendo o `config.toml` de verdade que o usuário tinha acabado de criar (com
  a chave real) em vez de falhar por falta de chave. Corrigido setando `HOME` explicitamente pra
  um diretório temporário único em `warden_command()`. **O mesmo gap existia em `warden-telegram/
  tests/telegram.rs` e `warden-whatsapp/tests/whatsapp.rs`** (nunca notado antes porque nenhuma
  das duas sessões anteriores tinha um `config.toml` real no ambiente pra vazar) — pior ainda
  nesses dois: uma chave Gemini real vazada faz o `bootstrap()` ter sucesso, aí o processo chega
  no loop de verdade (`run_bot`) com um token/sidecar fake — no Telegram isso significa retry
  infinito a cada 5s (por design, nunca desiste), no WhatsApp significa bloquear pra sempre
  esperando um evento do sidecar que nunca chega. Confirmado travando de verdade rodando a suíte
  completa (processo preso em `fails_clearly_without_a_gemini_key_once_the_token_is_present` por
  mais de 60s) antes da correção; corrigido nos três arquivos de teste com o mesmo padrão
- Testado: `cargo build --workspace`/`cargo test --workspace` limpos, 57 testes, suíte completa
  agora roda rápido de novo (sem travar)
- `~/.cargo/bin/warden` reinstalado via `cargo install --path crates/warden-cli` pra pegar os
  dois fixes (modelo novo + UX da Sessão 31, que o usuário ainda não tinha testado de verdade
  porque a instalação anterior era de antes dessas mudanças)
- `project/PENDING.md`/`ARCHITECTURE.md` **não** atualizados nesta sessão — são bugfixes
  pontuais, não decisões de arquitetura novas; o registro fica só aqui no log de sessões

**Próximo passo**: Usuário revogar a API key exposta no chat e gerar uma nova. Testar `warden` de
novo com o modelo corrigido e a UX nova (histórico, spinner, markdown) juntos pela primeira vez.

---

### 2026-08-09 — Sessão 31

- **Objetivo**: UX de terminal do `warden-cli` estilo Claude Code/Codex, em modo `/plan` —
  pedido do usuário depois de instalar e testar o `warden` de verdade via `cargo install`.

**O que foi feito**:

- Pergunta de escopo feita antes de planejar: entre 3 níveis (cores+markdown+histórico / + streaming
  de resposta real / TUI completo com `ratatui`) — usuário escolheu o primeiro, mais contido
- Achado de investigação que virou a decisão de design central: os 4 testes de processo de
  `tests/cli.rs` rodam com stdin/stdout pipados, não um TTY de verdade — `rustyline` em modo raw
  não funciona direito nesse cenário. Resolvido com `std::io::IsTerminal` (já na std, sem
  dependência nova): stdin não-TTY cai no loop simples de sempre (`run_plain`), preservando os 4
  testes sem tocar neles
- Implementado: novo módulo `crates/warden-cli/src/interactive.rs`, usado só quando
  `io::stdin().is_terminal()` — `rustyline::DefaultEditor` (histórico de linha persistido em
  `~/.config/warden/cli_history.txt`, Ctrl+C cancela a linha em vez de derrubar o processo,
  Ctrl+D encerra), `indicatif::ProgressBar` como spinner "Thinking..." enquanto o modelo responde,
  `termimad::MadSkin` renderizando a resposta como markdown de verdade (negrito, listas, code
  block) em vez de `println!` cru
- Durante a implementação, uma suposição do plano se mostrou errada ao verificar o `Cargo.toml`
  real do `termimad@0.35`: ele não depende mais de `crossterm` diretamente (usa `crokey`/`coolor`
  agora) — a ideia original de "reusar crossterm como transitiva do termimad" pra colorir a linha
  de uso de tokens não se sustentava. Corrigido na hora, trocado por `owo-colors`, uma dependência
  dedicada e mínima
- Verificado de ponta a ponta com um TTY de verdade (não só os testes com pipe): usado `script`
  (aloca um pseudo-terminal real) pra confirmar que o caminho rico realmente roda — prompt,
  spinner animando (frames reais capturados no log), histórico. Um primeiro teste deu timeout
  porque as duas linhas de input foram despejadas rápido demais no pty antes do processo estar
  pronto pra ler a segunda — não é bug de verdade, confirmado repetindo com um `sleep` entre as
  linhas: `exit` encerra limpo (exit code 0)
- Testado: `cargo build --workspace`/`cargo test --workspace` limpos, 57 testes (os 5 de
  `warden-cli` continuam passando sem nenhuma alteração neles, confirmando que a detecção de TTY
  funcionou como planejado)
- `project/PENDING.md` (P8 atualizada — primeiro passo de UX dado, streaming/TUI completo
  seguem em aberto), `project/ARCHITECTURE.md` (3 decisões novas: nível de ambição, libs
  escolhidas incluindo a correção do `owo-colors`, detecção de TTY)

**Próximo passo**: Verificação manual pendente pro usuário — rodar `warden` de verdade com uma
API key real, confirmar visualmente o histórico (seta pra cima), o spinner e o markdown
renderizado. P8 segue com streaming de resposta real e TUI completo como possíveis próximos
passos se fizerem falta na prática.

---

### 2026-08-09 — Sessão 30

- **Objetivo**: Correção rápida — feedback real do usuário depois de rodar a Fase 3 (Sessão 29)
  na própria máquina: o QR code aparecia visivelmente esticado no terminal — uma câmera comum
  conseguia ler, mas o scanner do próprio WhatsApp não reconhecia.

**O que foi feito**:

- Causa raiz: `qrcode-terminal` usa o truque de meio-bloco Unicode (▀▄) pra "comprimir" o QR
  verticalmente, assumindo uma proporção de fonte de terminal específica (~2:1 altura:largura).
  Quando essa suposição não bate com a fonte real do terminal, o QR sai distorcido — tolerável
  pra um leitor de QR genérico (mais robusto), não pro scanner específico do WhatsApp
- Trocado `qrcode-terminal` por `qrcode` (`QRCode.toFile`) em `sidecar/whatsapp/index.mjs`: gera
  um PNG de verdade (512×512, pixels quadrados, sem depender de fonte nenhuma) em
  `<authDir>/qr.png`, com o caminho logado no stderr do sidecar (mesma separação de canal já
  usada antes — nunca no stdout, que é o protocolo JSON-lines)
- Verificado de ponta a ponta de novo: `node --check` limpo, rodada real contra os servidores do
  WhatsApp confirma PNG válido gerado (`file` confirma 512×512 RGBA8), stdout continua em 0 bytes
- Também corrigida uma imprecisão factual encontrada nos docs da Sessão 29: `ARCHITECTURE.md`/
  `PHASE.md`/`SESSIONS.md` diziam que `baileys` tinha sido fixado na tag estável `^6.7.24` — na
  verdade essa era só a intenção original, nunca chegou a instalar de verdade (bloqueado pelo
  fetch de dependência git do `libsignal` no ambiente de verificação); o que de fato foi testado,
  commitado e agora confirmado rodando na máquina real do usuário é `^7.0.0-rc14`. Corrigido nos
  três arquivos pra refletir o que realmente está no `package.json`

**Próximo passo**: Usuário vai rodar `npm install` de novo (troca de dependência) e escanear o
novo QR em PNG pra validar o pareamento de verdade.

---

### 2026-08-09 — Sessão 29

- **Objetivo**: Fase 3 — Canal WhatsApp, em modo `/plan`.

**O que foi feito**:

- Duas perguntas de escopo feitas antes de planejar: (1) transporte IPC entre o sidecar Node
  (Baileys) e o core Rust — `PHASE.md` deixava em aberto "stdin/stdout ou socket", e não existia
  nenhum precedente de IPC customizado no repo (só o `TokioChildProcess` do MCP, específico do
  protocolo MCP) → usuário escolheu **stdin/stdout, JSON-lines**; (2) tratamento de mídia (etapa
  3.7) — suporte multimodal de verdade tocaria `ModelProvider`/`Message` (mudança de model-layer)
  → usuário escolheu **só degradação graciosa**
- Pesquisa da Bot API/lib do WhatsApp: confirmado `baileys` (fork mantido por `WhiskeySockets`,
  o `adiwajshing/Baileys` original está arquivado) como a lib certa; API real (`makeWASocket`,
  `useMultiFileAuthState`, `connection.update`/`DisconnectReason`, `messages.upsert`,
  `sock.sendMessage`) confirmada via README/exemplo oficial
- Implementado: `sidecar/whatsapp/` — **primeiro código JS que o próprio projeto escreve e
  versiona** (até aqui todo uso de Node era via `npx` contra pacotes de terceiros). `package.json`
  + `index.mjs` puro (ESM, sem TypeScript/build step). Intenção inicial era fixar `baileys` na
  tag estável `^6.7.24`, mas seu `libsignal` é resolvido via `git+https://...` e o ambiente de
  verificação bloqueia fetch de dependência git — trocado por `^7.0.0-rc14` (`libsignal` normal
  do registry), que de fato instalou e conectou; trade-off (pre-release pré-1.0) registrado em
  `ARCHITECTURE.md`
- Novo crate `crates/warden-whatsapp`, mesmo padrão do `warden-telegram`: trait `WhatsAppSidecar`
  (`&mut self`, diferente da `TelegramApi` que é `&self` — ler linha a linha de um stream é
  estado), `ChildSidecar` implementa de verdade sobre `tokio::process`/`tokio::io` (nova feature
  `io-util` do tokio, só nesta crate — primeira vez que o workspace precisa dela), `run_bot`/
  `handle_event` reusando `warden_bootstrap::handle_turn` da Fase 2 (novo
  `default_whatsapp_conversations_dir`, mesmo padrão do Telegram)
- **Dois bugs reais encontrados e corrigidos via verificação de ponta a ponta contra os
  servidores reais do WhatsApp** (não mockado): (1) `qrcode-terminal.generate()` sem callback
  escreve o QR via `console.log` — ou seja, no **stdout**, corrompendo o protocolo JSON-lines
  usado pra IPC — corrigido passando um callback que escreve em `process.stderr` explicitamente,
  confirmado depois com stdout/stderr capturados separadamente (0 bytes no stdout, QR limpo no
  stderr); (2) um smoke test (`fails_clearly_when_node_is_not_on_path`) travou a suíte inteira —
  `env_clear()` sozinho não impede o Linux de achar `node` (glibc cai pra um path default tipo
  `/bin:/usr/bin` quando `PATH` está totalmente ausente, não só vazio) — corrigido setando `PATH`
  pra um diretório que garantidamente não existe
- Verificado de ponta a ponta de verdade: `cargo run -p warden-whatsapp` com uma `GEMINI_API_KEY`
  fake sobe o orchestrator, spawna o sidecar real, o sidecar conecta nos servidores do WhatsApp e
  gera um QR de pareamento real — faltou só alguém escanear com o celular pra completar o pareamento
- P20 (trait `Channel`, deixada em aberto na Fase 2) fechada nesta sessão: com dois canais reais
  agora, confirmado que o que é compartilhável já está em `handle_turn`, os loops de recebimento
  em si continuam diferentes o bastante pra uma trait não valer a pena
- Testado: `cargo build --workspace`/`cargo test --workspace` limpos, 57 testes no total (6 novos
  desta sessão: 3 hermáticos em `sidecar.rs` via `ScriptedSidecar` + 3 smoke tests de processo)
- `project/PHASE.md` (Fase 3 completa, 3.1-3.8), `project/OVERVIEW.md`/`ROADMAP.md` (status),
  `project/ARCHITECTURE.md` (5 decisões novas: IPC, protocolo, código JS no repo, mídia, P20
  fechada), `project/PENDING.md` (P20 movida pra Resolvidas, P21 nova pro suporte multimodal —
  registrando também que o Telegram hoje ignora mídia **silenciosamente**, pior que a degradação
  graciosa nova do WhatsApp)

**Próximo passo**: Fases 2 e 3 concluídas. Setup manual pendente pro usuário: `npm install` em
`sidecar/whatsapp/` (já feito nesta sessão pra verificação, mas roda numa sandbox — o ambiente
real do usuário precisa do próprio `npm install`), depois `warden-whatsapp` + escanear o QR com o
celular pra validar uma conversa de verdade ponta a ponta. Seguem pendentes: Fase 4 (Vault &
IPFS), 5.4 (tool `browser`, depende da Fase 8), P21 (multimodal, nova).

---

### 2026-08-09 — Sessão 28

- **Objetivo**: Fase 2 — Canal Telegram, em modo `/plan`.

**O que foi feito**:

- Duas perguntas feitas ao usuário antes de planejar: (1) `PHASE.md` (etapa 2.2) pede uma trait
  `Channel`, mas investigação confirmou que nem `warden-cli` nem o desktop compartilham hoje
  nenhuma abstração de canal — cada um só chama `bootstrap()` e monta seu próprio loop. Trait com
  um único implementador validaria pouco → usuário escolheu **sem trait ainda**, com uma função
  reutilizável de "turn" no lugar, revisitar quando o WhatsApp (Fase 3) der um segundo exemplo
  real; (2) formatação MarkdownV2 nas respostas (etapa 2.5) tem regra de escape própria que
  quebra a chamada inteira da API se sair errado → usuário escolheu **texto puro** por enquanto
- Pesquisa da Bot API do Telegram (long polling via `getUpdates`, limite de 4096 caracteres por
  `sendMessage`, autenticação via token na URL) e exploração do código (`Orchestrator` é `Clone`
  barato, `bootstrap()` é a única abstração compartilhada hoje, `Conversation`/
  `ConversationMessage`/`save_conversation`/`list_conversations` já são genéricos o bastante pra
  reusar sem mudança)
- Implementado: novo crate binário `crates/warden-telegram` (`warden-telegram`), mesmo padrão do
  `warden-cli` (clap derive, `bootstrap()`, vault default `~/Warden/vault` como o desktop já usa
  pra processos sem cwd previsível); `crates/warden-bootstrap` ganhou `load_conversation` (versão
  "uma conversa só" do `list_conversations` existente), `handle_turn` (a função reutilizável
  decidida na pergunta 1 — carrega histórico, chama o orchestrator, persiste, pronta pro WhatsApp
  reusar depois) e `default_telegram_conversations_dir` (diretório próprio, não mistura com o que
  o sidebar do desktop lista — ver nota nova em `ROADMAP.md`); `ApiKeys` ganhou
  `telegram_bot_token`
- Bug real encontrado e corrigido de passagem: `save_settings` (desktop) já tinha o padrão de
  "carregar o config existente e preservar campos sem UI" pra `mcp_servers`, mas não fazia isso
  pro `telegram_bot_token` novo — sem a correção, salvar as Settings do desktop apagaria
  silenciosamente um token hand-editado no `config.toml`. Corrigido reusando o mesmo `existing`
  já carregado
- `crates/warden-telegram/src/telegram.rs`: trait fina `TelegramApi` (`get_updates`/
  `send_message`) implementada de verdade por `TelegramClient` contra a API real, e mockável em
  teste por `ScriptedTelegramApi` — mesmo espírito do `ModelProvider`/`ScriptedModel` já usado em
  `pipeline.rs`, sem introduzir `wiremock`/dependência nova só pra isso. `run_bot`/
  `process_updates`/`handle_update` genéricos sobre `impl TelegramApi`; `/start`/`/help`
  respondem sem chamar o orchestrator; mensagem longa é dividida em pedaços ≤4096 bytes
  respeitando fronteira de char (mesma técnica de `tool/shell.rs::truncate`); erro de rede no
  polling loga e tenta de novo em 5s em vez de derrubar o processo
- Testado: 5 testes hermáticos em `telegram.rs` (turno completo com persistência, offset avança
  sem reprocessar, `/start`/`/help` não chama o orchestrator, split de mensagem longa em 2
  pedaços, split respeita fronteira de char UTF-8) + 4 smoke tests de processo em `tests/
  telegram.rs` (falha clara sem `TELEGRAM_BOT_TOKEN`, falha clara sem `GEMINI_API_KEY` já com o
  token presente, token lido do config file, `--config` apontando pra arquivo inexistente) —
  todos os cenários falham antes de qualquer chamada de rede real, mesmo espírito do `cli.rs`.
  `cargo build --workspace`/`cargo test --workspace` limpos (51 testes no total)
- **Sem `TELEGRAM_BOT_TOKEN` real disponível neste ambiente** — não deu pra validar o long
  polling de verdade (mandar uma mensagem real pro bot e ver a resposta chegar); fica como
  verificação manual pendente pro usuário, criar um bot via `@BotFather` é rápido
- `project/PHASE.md` (Fase 2 completa, 2.1-2.8, com notas nas etapas 2.2 e 2.5 sobre o escopo
  reduzido), `project/OVERVIEW.md` (status geral), `project/ARCHITECTURE.md` (5 decisões novas:
  sem trait Channel, cliente HTTP testável via trait fina, texto puro, diretório de conversas
  separado, long polling vs webhook), `project/ROADMAP.md` (item 3 marcado concluído, nova nota
  de brainstorm sobre visão unificada de conversas entre canais)

**Próximo passo**: Fase 2 concluída. Seguem pendentes: Fase 3 (WhatsApp — primeira candidata a
reusar `handle_turn`/dar o segundo exemplo real pra decidir se vale uma trait `Channel` de
verdade), Fase 4 (Vault & IPFS), 5.4 (tool `browser`, depende da Fase 8). Verificação manual
pendente: rodar `warden-telegram` com um `TELEGRAM_BOT_TOKEN` real e confirmar o long polling
funcionando de ponta a ponta.

---

### 2026-08-09 — Sessão 27

- **Objetivo**: Etapa 5.8 — "Rate limiting e controle de custo por tool", em modo `/plan`.

**O que foi feito**:

- Pergunta feita ao usuário antes de planejar (a etapa mistura dois mecanismos distintos): tracking
  de uso vs tracking + teto rígido de custo vs rate limiting por tool vs os dois → usuário escolheu
  **só tracking de uso**, sem limites/bloqueios ainda
- Duas rodadas de exploração (agentes `Explore`) confirmaram: (1) `Orchestrator::handle_message` é
  o único chokepoint por onde toda chamada de modelo passa — ponto certo pra capturar uso sem
  duplicar lógica por canal; (2) nem `OpenAiProvider` nem `GeminiProvider` liam o `usage`/
  `usageMetadata` que as APIs já devolvem — 100% descartado hoje, e o repo inteiro não tinha
  nenhum rastro de tracking de custo/token (`grep` vazio); (3) persistência de conversa
  (`ConversationMessage`) só existe no desktop, a CLI não persiste nada
- Implementado: novo tipo `Usage` (`warden_core::model`, `prompt_tokens`/`completion_tokens`/
  `total_tokens`, serde camelCase) e `Response.usage: Option<Usage>`; `OpenAiProvider` e
  `GeminiProvider` agora parseiam o campo de uso real de cada API; `Orchestrator::handle_message`
  passou a devolver `MessageOutcome{content, usage}` em vez de `String` cru, somando o uso das
  até 8 chamadas de modelo que uma única mensagem pode disparar; `ConversationMessage` ganhou
  `#[serde(default)] usage: Option<Usage>` (default garante que conversas já salvas em disco sem
  esse campo continuam carregando); desktop (`send_message`) e frontend (`types.ts`/`App.tsx`)
  atualizados pra propagar `usage` até um badge discreto de tokens em `MessageBubble.tsx` (só na
  mensagem do assistente); CLI ecoa uma linha de tokens após a resposta (sem persistir, já que a
  CLI nunca persistiu conversa)
- Gap encontrado e aceito conscientemente (documentado no código e como pendência nova, P18 em
  `PENDING.md`): `DelegateTool` chama `handle_message` recursivamente num sub-orchestrator, mas
  `Tool::call` só devolve `serde_json::Value` — o uso do sub-agente não sobe pro total da
  conversa pai. Fechar isso mudaria a trait `Tool` inteira, fora do escopo mínimo desta sessão
- Ajuste mecânico em todos os testes que quebraram com a mudança de assinatura (~12 literais
  `Response{...}` em `orchestrator/mod.rs`, `tool/delegate.rs` e `tests/pipeline.rs`, mais os
  `assert_eq!` que comparavam `handle_message(...)` direto contra `String`)
- Verificado: `cargo build --workspace` e `cargo test --workspace` limpos (42 testes, todos
  mockados — os nomes de campo das APIs OpenAI/Gemini são parte pública estável, não um mecanismo
  de terceiro a provar como nas sessões 5.3/5.6/5.7), `npx tsc --noEmit` limpo no frontend. **Sem
  chave de API real disponível neste ambiente** — não deu pra confirmar números de token reais
  ponta a ponta; fica como verificação manual pendente pro usuário
- `project/PHASE.md` (5.8 concluída, com nota do escopo reduzido), `project/ARCHITECTURE.md`
  (duas decisões novas: escopo e implementação), `project/PENDING.md` (P4 estreitada pro que
  resta — rate limiting/teto de gasto de verdade —, P18 nova registrando o gap do `DelegateTool`)

**Próximo passo**: Fase 5 fica só com 5.4 (tool `browser`, depende da Fase 8) em aberto — 5.1,
5.2, 5.3, 5.5, 5.6, 5.7 e 5.8 concluídas. P4 (rate limiting/teto de gasto de verdade) e P18
(uso do delegate) seguem como trabalho futuro se o usuário quiser fechar esse escopo depois.
Verificação manual pendente: rodar `warden`/app desktop com uma API key real e conferir a linha/
badge de tokens aparecendo de verdade.

---

### 2026-08-09 — Sessão 26

- **Objetivo**: Etapa 5.7 — Integração Google (Gmail/Drive/Calendar) via MCP servers existentes.

**O que foi feito**:

- Pesquisado o estado atual do ecossistema MCP pra Google: o server oficial de referência
  (`@modelcontextprotocol/server-gdrive`) está **arquivado**, sem manutenção — não existe mais
  opção oficial, diferente da Tavily (5.3) e do filesystem (5.6)
- Comparados os principais candidatos da comunidade por estrelas/atividade real no GitHub (via
  API, não só busca): `taylorwilsdon/google_workspace_mcp` (uvx/Python, 2987★, ativo, o mais
  completo — 120+ tools/12 serviços) vs `aaronsb/google-workspace-mcp` (npx/Node.js, 164★,
  ativo, 11 tools/7 serviços) vs `dguido/google-workspace-mcp` (**arquivado**, 38★) vs outros
  com pouquíssima tração (`danielrosehill` 1★, `j3k0` 32★)
- Pergunta feita ao usuário antes de codar (trade-off real, não óbvio): cobertura máxima
  (`taylorwilsdon`, mas introduz `uv`/Python como segundo runtime obrigatório) vs consistência
  de runtime (`aaronsb`, `npx`, mesmo runtime já aceito pro Tavily/filesystem, ver P17 em
  `PENDING.md`) → usuário escolheu **`aaronsb/google-workspace-mcp` (npx)**
- Confirmado, lendo `warden-bootstrap/src/lib.rs`, que o mecanismo genérico `[[mcp_servers]]`
  (name/command/args/env) já cobre esse caso sem nenhum código novo — mesma conclusão da 5.6
- **Verificação real, de ponta a ponta**, via `examples/verify_google_workspace_mcp.rs`
  descartável (removido depois, mesmo padrão da 5.3/5.6): conectado via
  `McpToolProvider::connect_stdio` de produção, **sem** `GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`
  setadas — o server sobe e responde `tools/list` normalmente mesmo sem credenciais (só falha
  depois, ao chamar uma tool de verdade), listou os 11 tools reais (`manage_email`,
  `manage_calendar`, `manage_drive`, `manage_docs`, `manage_sheets`, `manage_tasks`,
  `manage_meet`, `manage_accounts`, etc.)
- `project/PHASE.md` (5.7 concluída, sem código novo — mesma nota da 5.6), `project/
  ARCHITECTURE.md` (duas decisões novas: qual server e por quê, e reuso de `mcp_servers` com o
  exemplo de TOML pra habilitar + nota de que o setup OAuth no Google Cloud Console é 100% do
  lado do usuário, não automatizável pelo Warden), `project/PENDING.md` (P17 atualizada — 5.7
  confirmou a hipótese de mais uma dependência `npx`, e documentou por que foi escolhida de
  propósito em vez de evitada)

**Próximo passo**: Fase 5 segue com 5.4 (tool `browser`, depende da Fase 8/extensão — fora de
ordem) e 5.8 (rate limiting/custo por tool, ligado a P4). Segue em aberto a UI de P11
(gerenciamento visual de servers MCP — cobre agora `file_system` e Google também) e a
documentação de setup do usuário pra credenciais OAuth do Google (ainda só existe em
`ARCHITECTURE.md`, o projeto não tem README de usuário ainda).

---

### 2026-08-04 — Sessão 25

- **Objetivo**: Etapa 5.6 — Tool `file_system`. Continuação direta da sessão anterior (5.3),
  em modo `/plan`.

**O que foi feito**:

- Antes de planejar, notada uma ambiguidade real no próprio texto do `PHASE.md`: 5.6 diz
  "arquivos no **nó cliente**", diferente do escopo do `read_file`/`write_file` (Fase 1.6, só
  vault). Investigando `memory/mod.rs`, confirmado que `Vault::read`/`write` fazem só
  `root.join(path)` sem canonicalizar nem conter — path traversal não é bloqueado hoje nesses
  dois tools (mesma lacuna já anotada de passagem na investigação de segurança do `shell` na
  sessão 22, agora confirmada de novo no contexto certo)
- Duas perguntas feitas ao usuário antes de codar: (1) 5.6 é uma capacidade **nova e separada**
  (fora do vault) ou deveria **substituir** `read_file`/`write_file`? → **nova e separada**,
  os dois tools do vault continuam como estão; (2) inclui UI na Settings pra gerenciar
  diretórios permitidos já nesta sessão (o `tauri-plugin-dialog` já está registrado no app mas
  sem nenhum uso real ainda), ou fica só `config.toml` como os `mcp_servers` genéricos da 5.2?
  → **só config.toml** por enquanto
- Durante o design (decisão técnica, não perguntada — aplicação direta da diretriz do projeto
  contra abstração redundante): achado que o server MCP oficial de referência pra isso já
  existe, `@modelcontextprotocol/server-filesystem` (`npx -y @modelcontextprotocol/server-
  filesystem <dir1> [dir2...]`, confirmado no README oficial do repo `modelcontextprotocol/
  servers`), e que isso já é 100% expressável hoje via o mecanismo genérico `[[mcp_servers]]`
  da 5.2, sem precisar de nenhum campo de config novo (`file_system_allowed_dirs` seria
  redundante — diferente do Tavily, que ganhou tratamento dedicado por ter um "segredo" único
  e óbvio, `file_system` não tem equivalente natural pra virar config especial, é só uma lista
  de diretórios). **Conclusão: 5.6 não precisou de nenhum código novo em `warden-bootstrap`/
  `warden-core`** — só verificação real de que o server oficial funciona através do
  `McpToolProvider`, e documentação de como habilitar
- **Verificação real, de ponta a ponta**, via `examples/verify_filesystem_mcp.rs` descartável
  em `warden-core` (removido depois, mesmo padrão da 5.3): conectou no server real via
  `McpToolProvider::connect_stdio` de produção, listou **14 tools** (`read_file`,
  `read_text_file`, `write_file`, `edit_file`, `create_directory`, `list_directory`,
  `move_file`, `search_files`, `directory_tree`, `get_file_info`, etc. — bem mais rico que os
  2 tools do vault), escreveu um arquivo de verdade dentro de um diretório temporário permitido
  e leu de volta com sucesso (conteúdo bateu exato), e tentou escrever **fora** do diretório
  permitido — rejeitado pelo próprio server (`"Access denied - path outside allowed
  directories"`, confirmado com `Path::exists()` que o arquivo não foi criado). Prova concreta
  de que essa capacidade, além de mais rica, é mais segura que o `read_file`/`write_file` atual
- `project/PHASE.md` (5.6 concluída, com nota explicando que não há código novo — reuso
  deliberado do mecanismo da 5.2), `project/ARCHITECTURE.md` (duas decisões novas: escopo
  separado do vault, e reuso de `mcp_servers` em vez de campo de config dedicado — com o
  exemplo de TOML pra habilitar, que hoje é a única documentação de como usar isso já que o
  projeto ainda não tem um README de usuário)

**Próximo passo**: Fase 5 segue com 5.4 (tool `browser`, depende da Fase 8/extensão — fora de
ordem), 5.7 (integração Google via MCP servers — mais um caso provável de servers `npx`-based,
ver P17 em `PENDING.md`), e 5.8 (rate limiting/custo por tool). Segue também em aberto Fases
2-4 ou a UI de P11 (gerenciamento visual de servers MCP, que agora cobre tanto integrações
custom quanto o próprio `file_system`).

---

### 2026-08-04 — Sessão 24

- **Objetivo**: Etapa 5.3 — Tool `web_search` via MCP. Continuação direta da sessão anterior
  (5.1/5.2), escolhida pelo usuário entre seguir na Fase 5 vs Fases 2-4.

**O que foi feito**:

- Antes de implementar, investigado o que "via MCP" realmente significava aqui: `web_search`
  já existia desde a Fase 1.7, mas como uma tool Rust pura (`WebSearchTool`) chamando a API
  REST da Tavily direto via `reqwest`, implementada ad-hoc antes do client MCP (5.2) existir.
  Achado real via `WebSearch`/`WebFetch` (não assumido): a própria Tavily mantém um server MCP
  oficial, `tavily-mcp` (https://docs.tavily.com/documentation/mcp — `npx -y tavily-mcp`, env
  `TAVILY_API_KEY`, mesmo shape `command`/`args`/`env` que o `McpToolProvider` da 5.2 já
  suporta). Como isso significava **substituir** algo que já funcionava (não só adicionar algo
  novo), e o trade-off é real (passa a exigir Node.js/`npx` em runtime, algo que o Warden não
  precisava antes), perguntado ao usuário entre três caminhos — substituir de vez, manter os
  dois, ou pular pra 5.6. Escolhido: **substituir**
- Removido `crates/warden-core/src/tool/web_search.rs` inteiro (`WebSearchTool` + seu teste)
  e a entrada `pub mod web_search;` em `tool/mod.rs`. `reqwest` continua no `Cargo.toml` do
  `warden-core` — ainda usado por `model/openai.rs`/`model/gemini.rs`, não só pelo que saiu
- `crates/warden-bootstrap/src/lib.rs`: novo helper privado `register_mcp_server_tools`
  (connect→list→extend→degradação graciosa) — extraído porque, com essa mudança, o padrão
  passou a ter dois call sites reais e idênticos (o gate do Tavily e o loop de
  `config.mcp_servers` da 5.2), não é abstração especulativa. O gate do Tavily
  (`resolve_secret(TAVILY_API_KEY, ...)`) continua igual, só que agora, em vez de construir
  `WebSearchTool` direto, chama `register_mcp_server_tools(..., "tavily", "npx", ["-y",
  "tavily-mcp"], [("TAVILY_API_KEY", key)])`. Mensagem do branch `None` mantém a substring
  `"TAVILY_API_KEY"` de propósito — o teste `warden-cli/tests/cli.rs::starts_up_and_exits_
  cleanly_with_a_key_present` já checa isso no stderr e não precisou mudar
- **Verificação real contra o server publicado de verdade**, não só o de teste da 5.2: sem
  `TAVILY_API_KEY` de verdade nesta máquina (mesma limitação já registrada nas sessões
  anteriores pra Gemini/OpenAI), primeiro confirmado com `npx tavily-mcp --list-tools` (flag
  própria do pacote) que o server sobe e lista tools mesmo com key placeholder — listar não
  exige key válida, só chamar. Depois, escrito um `examples/verify_tavily_mcp.rs` descartável
  em `warden-core` (removido logo depois, não ficou no repo — dependeria de rede/`npx`, tornaria
  o `cargo test` frágil em CI) chamando o próprio `McpToolProvider::connect_stdio` de produção
  contra `npx -y tavily-mcp` de verdade: conectou, completou o handshake MCP e listou as 5
  tools reais (`tavily_search`, `tavily_extract`, `tavily_crawl`, `tavily_map`,
  `tavily_research`) com descrições — prova de ponta a ponta que o client MCP da 5.2 funciona
  contra um server de terceiro publicado, não só contra o server escrito à mão pro teste
- Verificação: `cargo build/clippy --workspace --all-targets` limpos; `cargo test --workspace`
  verde (contagem de testes do `warden-core` caiu em 1 com a remoção de
  `requires_query_argument`, o resto sem mudança)
- `project/PHASE.md` (5.3 concluída; nota adicionada em 1.7 apontando que a implementação
  original foi substituída), `project/ARCHITECTURE.md` (decisão da substituição REST→MCP
  registrada, com o trade-off do Node.js explícito), `project/PENDING.md` (novo item P17 —
  dependência de Node.js/`npx` em runtime, hoje só com erro cru do SO se ausente; vai ficar
  mais relevante ainda na 5.7)

**Próximo passo**: Fase 5 segue com 5.4 (tool `browser`, depende da Fase 8/extensão — fora de
ordem, provavelmente pular por ora), 5.6 (`file_system` via MCP — hoje `read_file`/`write_file`
já existem como tools Rust puras da Fase 1.6, mesma pergunta de "substituir vs manter" que
surgiu aqui pode se repetir), 5.7 (integração Google via MCP servers — provavelmente outro
server `npx`-based, caso real pro P17) e 5.8 (rate limiting). Segue também em aberto continuar
Fases 2-4 ou a UI de P11 (gerenciamento visual de servers MCP), como já estava na sessão
anterior.

---

### 2026-08-04 — Sessão 23

- **Objetivo**: Etapas 5.1 e 5.2 — registry de tools (`ToolProvider` trait) + MCP client
  (conectar em servers MCP externos). Escolhido pelo usuário entre continuar a Fase 5 vs
  atacar Fases 2/4 vs outras pendências.

**O que foi feito**:

- `crates/warden-core/src/tool/mod.rs` ganhou a trait `ToolProvider` (`async fn tools(&self)
  -> anyhow::Result<Vec<Arc<dyn Tool>>>`) — motivada pelo MCP: diferente de um `Tool` fixo
  compilado no binário, conectar num server MCP só revela seu conjunto de tools em runtime
  (`tools/list`, depois do handshake). `Orchestrator::register_provider` (novo método) chama
  `tools()` e registra cada uma via `register_tool` já existente — é um snapshot no momento da
  chamada, sem live-sync
- Decisão de SDK: **`rmcp`** (crates.io, mantido pela org `modelcontextprotocol`) em vez de
  implementar o handshake JSON-RPC do MCP na mão — protocolo já padronizado, reimplementar
  seria retrabalho puro. Investigação da API feita direto no source baixado pelo cargo
  (`~/.cargo/registry/src/.../rmcp-3.1.0`), já que é uma lib nova no projeto: `TokioChildProcess`
  (transporte stdio/child-process), `().serve(transport)` pra abrir a sessão, `list_all_tools()`/
  `call_tool()` no client
- `crates/warden-core/src/tool/mcp.rs` (novo) — `McpToolProvider::connect_stdio(server_name,
  command, args, env)`: spawna `command args...` como processo filho e faz o handshake MCP.
  `env` é passado via `Command::envs` (escopado só ao processo filho, nunca muta o processo do
  Warden) — mesmo shape que qualquer client MCP usa (`command`/`args`/`env`, igual ao
  `mcpServers` do Claude Desktop), pensado pra portar configs existentes quase verbatim.
  `ToolProvider::tools()` lista as tools do server e devolve cada uma envolvida num adapter
  interno (`McpTool`) que implementa `Tool` encaminhando `call()` como `tools/call` pra sessão
  compartilhada (`Arc<RunningService<RoleClient, ()>>`)
- **Verificação real, não mockada**: `crates/warden-core/tests/mcp_stdio.rs` sobe um MCP server
  de verdade (`EchoServer`, via `ServerHandler` do próprio `rmcp`) como processo filho de
  verdade falando o protocolo stdio real — mesmo truque de self-re-exec que o próprio test
  suite do `rmcp` usa (`test_stdio_response_concurrency.rs` upstream): o binário de teste
  reinvoca a si mesmo com `--exact mcp_stdio_test_helper`, e essa segunda instância vira o lado
  servidor da sessão. O teste conecta via `McpToolProvider::connect_stdio` de verdade, lista as
  tools (`echo`), chama a tool e confere o resultado — é a mesma tool que o `warden-bootstrap`
  vai chamar em produção, não um mock
- **Bug de corrida real encontrado e corrigido durante a verificação**: a primeira versão do
  teste passava a env var do helper via `std::env::set_var` no processo pai (mutando o processo
  inteiro), não escopada ao filho. Rodando o binário de teste diretamente (fora do `cargo test`,
  pra depurar um `error: io error when listing tests: Broken pipe` que aparecia sempre), ficou
  claro que os dois `#[tokio::test]` do arquivo rodam **no mesmo processo**, em paralelo por
  padrão — então a env var vazava pro teste `mcp_stdio_test_helper` mesmo quando ele deveria ser
  um no-op, fazendo-o tentar virar servidor MCP em cima do stdio real do processo (que não é um
  client MCP), falhando com "connection closed: initialize request". Corrigido adicionando
  suporte a `env` de verdade em `connect_stdio` (passado via `Command::envs`, só pro filho) —
  que também é uma feature real de produção (servers MCP frequentemente precisam de env vars,
  ex. API keys), não só um ajuste de teste. O `error: io error when listing tests: Broken pipe`
  em si **continua aparecendo** mesmo depois da correção, de forma determinística e inofensiva —
  investigado a fundo: é o processo filho (que é o próprio binário de teste) tentando imprimir o
  resumo do seu único teste no stdout (compartilhado como transporte MCP) bem na hora em que o
  `Drop` do `McpToolProvider` mata o processo, perdendo a corrida contra o `kill_on_drop`. Não
  afeta a troca JSON-RPC (já concluída nesse ponto) nem o resultado do teste (sempre verde,
  confirmado em 5+ execuções seguidas) — documentado com comentário extenso no próprio arquivo
  de teste em vez de resolvido com uma API de shutdown gracioso que nada mais no projeto
  precisaria ainda
- `crates/warden-core/Cargo.toml`: `rmcp` com features `client` + `transport-child-process` em
  `[dependencies]` (o que vai pro binário real); `transport-io` só em `[dev-dependencies]`
  (usado exclusivamente pelo lado servidor do teste acima)
- `crates/warden-bootstrap/src/lib.rs`: `FileConfig` ganhou `mcp_servers: Vec<McpServerConfig>`
  (`#[serde(default)]`, TOML como `[[mcp_servers]]` com `name`/`command`/`args`/`env`).
  `bootstrap()` conecta em cada server configurado e registra as tools que ele expõe — falha
  em conectar ou listar não derruba o app, só loga um aviso e pula aquele server (mesmo espírito
  de degradação graciosa do `TAVILY_API_KEY`/shell). Isso fechou o caso concreto do usuário: o
  lado Warden pra conectar no Anchor/TruthID via MCP (P11/P13) já está pronto, falta só esses
  outros projetos exporem um server MCP
- **Efeito colateral em cascata**: `bootstrap()` precisou virar `async fn` (spawnar processo +
  aguardar handshake do MCP não dá pra fazer de forma síncrona). Isso tocou os três chamadores:
  `warden-cli/src/main.rs` (trivial, `main` já era `#[tokio::main] async fn`); `desktop/src-tauri/
  src/lib.rs` — `save_settings` virou `#[tauri::command] async fn` (mesmo padrão já usado por
  `send_message`); `run()` (chamado *antes* do runtime do Tauri iniciar, então sem `.await`
  disponível) precisou de `tauri::async_runtime::block_on(bootstrap(...))`. Aproveitando essa
  passada em `save_settings`: como a tela de Settings não tem UI pra `mcp_servers` ainda (só
  editável a mão no `config.toml` — é a parte de UI do P11, ainda em aberto), o handler agora
  carrega o config existente antes de sobrescrever e propaga o `mcp_servers` dele, em vez de
  resetar pra vazio a cada "Salvar" — sem isso, salvar qualquer outra configuração pela tela
  apagaria silenciosamente servers MCP editados a mão
- Verificação: `cargo build/clippy --workspace --all-targets` limpos; `cargo test --workspace`
  verde (43 testes no total — 19 no `warden-core` incluindo o novo `register_provider_registers_
  every_tool_it_yields`, 14 no `warden-bootstrap` incluindo `parses_mcp_servers_from_toml` e o
  round-trip de `save_config` agora cobrindo `mcp_servers`, mais os 2 do `mcp_stdio.rs` contra
  processo real)
- `project/PHASE.md` (5.1 e 5.2 concluídas), `project/ARCHITECTURE.md` (4 decisões novas: registry
  dinâmico, SDK `rmcp`, transporte stdio, formato de config de servers), `project/PENDING.md`
  (P11 e P13 atualizadas — o lado Warden do client MCP está pronto, falta UI em P11 e os
  projetos externos exporem MCP em P13)

**Próximo passo**: decisão em aberto entre continuar a Fase 5 (5.3 `web_search` via MCP — hoje é
direto via API Tavily, não MCP; 5.6 `file_system` via MCP; 5.7 integração Google via MCP servers
existentes — provavelmente o próximo caso de uso real de `mcp_servers`; 5.8 rate limiting) vs
atacar as Fases 2-4 (Telegram, WhatsApp, Vault+IPFS, todas ainda pendentes) vs a UI de P11
(tela de gerenciamento de servers MCP na Settings do desktop). Vale também o usuário testar de
verdade conectando num server MCP real (ex. `npx -y @modelcontextprotocol/server-filesystem
<dir>` ou o próprio Anchor/TruthID se já tiverem um) — a verificação desta sessão usou um server
MCP real, mas escrito à mão como parte do teste, não um server de terceiro rodando via `npx`.

---

### 2026-08-04 — Sessão 22

- **Objetivo**: Etapa 5.5 — tool `shell`, escolhida como "ganho rápido" depois de fechar a
  Fase 6 (a trait `Tool` já existia com dois exemplos reais, sem precisar do registry/MCP da
  5.1/5.2).

**O que foi feito**:

- Investigação prévia (`PENDING.md` P8, `ROADMAP.md`, `ARCHITECTURE.md`, `GUIDELINES.md`, código
  de `file_tools.rs`/`web_search.rs`) mostrou que o projeto não tinha **nenhuma** decisão de
  segurança pra uma tool desse tipo — as tools de arquivo já não têm scoping real (path
  traversal não é bloqueado), então não havia um modelo de sandboxing pra replicar, e uma tool
  de shell é categoricamente mais arriscada. Perguntado ao usuário antes de implementar (não é
  decisão que a IA deveria tomar sozinha): (1) opt-in desligada por padrão vs sempre ativa como
  as outras tools — escolhido **opt-in**; (2) toggle na Settings UI do desktop já nesta sessão
  vs só config.toml/env por enquanto — escolhido **incluir agora**, o que expandiu o escopo
  pra tocar `desktop/src-tauri` e o frontend também
- `crates/warden-core/src/tool/shell.rs` (novo) — `ShellTool`, mesma forma estrutural de
  `file_tools.rs`: `spec()` com JSON Schema plano (primeiro caso do projeto com parâmetro
  numérico, `timeout_ms`, não só string), `call()` com extração manual de args. Roda via
  `sh -c` (Unix, `#[cfg(not(target_os = "windows"))]`) ou `cmd /C` (Windows) — cobre a
  diferença já anotada no `ROADMAP.md`; PowerShell fica fora de escopo. `tokio::process::Command`
  com `.kill_on_drop(true)` + `tokio::time::timeout` — timeout mata o processo e devolve
  `{"timed_out": true}` em vez de erro, pro modelo conseguir reagir. `cwd` default = raiz do
  vault (`Vault::root()`), overridável relativo ou absoluto, criado via `create_dir_all` se
  não existir ainda. `stdout`/`stderr` truncados em ~20KB respeitando char boundary UTF-8, pra
  não estourar o contexto do modelo com output gigante. `tokio` saiu de `dev-dependencies` pra
  `dependencies` de verdade em `warden-core` (só era usado em `#[tokio::test]` antes), e o
  workspace ganhou as features `"process"`/`"time"` na entrada compartilhada
- `crates/warden-bootstrap/src/lib.rs`: `FileConfig` ganhou `enable_shell: Option<bool>`; nova
  `resolve_flag(from_env, from_file) -> bool` espelha `resolve_secret` mas parseia o env var
  como booleano (`"1"`/`"true"`/`"yes"`). Gate em `bootstrap()` no mesmo ponto do gate do
  Tavily — liga se `resolve_flag(WARDEN_ENABLE_SHELL, config.enable_shell)`, senão `eprintln!`
  explicando como ligar
- Desktop: `SettingsSnapshot`/`SettingsFormPayload` (`src-tauri/src/lib.rs`),
  `Settings` (`types.ts`) e `SettingsView.tsx` ganharam `enable_shell`/`enableShell` — checkbox
  novo com aviso de risco embaixo ("lets the model run any command on this machine, with no
  sandboxing"), `App.css` com estilo mínimo (`accent-color` roxo pra bater com o tema).
  Salvar já dispara o `bootstrap()` de novo (mecanismo da 6.5, de graça) — ligar/desligar a
  tool na Settings faz live-reload sem reiniciar o app
- **Bug real encontrado e corrigido durante a verificação visual**: o primeiro `onChange` do
  checkbox lia `e.currentTarget.checked` *dentro* do updater function passado pro `setForm`
  (`setForm((f) => ({ ...f, enableShell: e.currentTarget.checked }))`) — mesmo padrão usado nos
  outros campos de texto do formulário, mas pra checkbox isso quebrava com
  `TypeError: Cannot read properties of null (reading 'checked')` assim que clicado de verdade
  (só apareceu testando clique real via Playwright — `page.fill()` não reproduz, por isso não
  foi pego só de olhar o código). Corrigido extraindo `e.currentTarget.checked` numa `const`
  **antes** de chamar `setForm`, em vez de ler o evento de dentro do closure do updater
- Verificação real: `cargo test --workspace` (18 testes novos/atualizados no `warden-core`,
  incluindo um que mata de verdade um `sleep 5` com `timeout_ms: 50` e confere
  `timed_out: true` — subprocess real, não mock; mais `resolve_flag` no `warden-bootstrap`),
  `cargo clippy --workspace --all-targets` limpo, `npm run build` limpo. UI testada de verdade
  via Chromium headless (Playwright, instalado e removido só pro teste, mesmo padrão da sessão
  20): mock do `get_settings`/`save_settings`, screenshot antes/depois do clique no checkbox,
  payload de `save_settings` conferido (`enable_shell: true`) — foi esse teste que pegou o bug
  do `currentTarget` acima. Sem `GEMINI_API_KEY`/`OPENAI_API_KEY` configurada nesta máquina, não
  deu pra testar o fluxo "modelo decide chamar a tool shell" de ponta a ponta com um modelo de
  verdade — os testes unitários já prova que a tool em si funciona (processo real); fica pro
  usuário confirmar com uma key configurada (`WARDEN_ENABLE_SHELL=1`)
- `project/ARCHITECTURE.md` (decisão de segurança da 5.5 registrada), `project/PHASE.md` (5.5
  concluída), `project/PENDING.md` (P8 atualizada — a tool em si não é mais o que falta ali,
  só a arquitetura do canal Terminal dedicado)

**Próximo passo**: escolher entre continuar na Fase 5 (5.1/5.2 registry+MCP client, que
destravam P11/P13) ou atacar as Fases 2-4, ainda todas pendentes.

---

### 2026-08-04 — Sessão 21

- **Objetivo**: Etapa 6.8 — build Linux/Windows/macOS. Última etapa da Fase 6 (App Desktop
  Nativo).

**O que foi feito**:

- Build multi-plataforma de verdade pra um app Tauri não dá pra fazer local (sem SDK da Apple
  nem toolchain Windows nesta máquina Arch) — o caminho é CI, cada OS buildando num runner
  nativo. Antes de desenhar algo novo, explorados em paralelo (a) o próprio
  `desktop/src-tauri/tauri.conf.json` do Warden e (b) o precedente do TruthID
  (`truthid/.github/workflows/build.yml`), que já resolve exatamente esse problema pro mesmo
  stack (Tauri v2). `GUIDELINES.md` pede consistência com os outros projetos — decisão foi
  replicar o `build.yml` do TruthID quase literalmente, adaptado pros nomes/paths do Warden
- Criado `.github/workflows/build.yml` — primeira automação de CI do repo. Dispara em tag `v*`,
  matrix `ubuntu-22.04` (→ deb)/`windows-latest` (→ msi)/`macos-latest` (→ dmg), usa
  `tauri-apps/tauri-action@v0` com `releaseDraft: true`, sem assinatura de código nem
  notarização macOS (mesma decisão já tomada no TruthID). Única diferença real do original:
  `npm ci` sem `--legacy-peer-deps` — testado local e confirmado que o `package.json` do Warden
  não precisa da flag (mais simples que o do TruthID)
- `tauri.conf.json` não precisou de nenhuma mudança — já está estruturalmente idêntico ao do
  TruthID (`bundle.targets: "all"`, ícones já gerados em `icons/`)
- **Verificação real, não só leitura de doc**: rodei `npx tauri build` de verdade nesta máquina
  (o mesmo que o job `ubuntu-22.04` do workflow faz) — compilou o workspace inteiro em release
  (~8min) e gerou `.deb`/`.rpm` válidos em `target/release/bundle/`. Prova de ponta a ponta que
  o pipeline de build funciona. O bundle de `.AppImage` falhou (`failed to run linuxdeploy`) —
  investigado a fundo em vez de só anotar como "não deu": não é FUSE ausente (`/dev/fuse`
  existe, `--appimage-mount` monta normal), é o `strip` **embutido dentro do
  `linuxdeploy-x86_64.AppImage`** (binutils de meados de 2024) que não reconhece a seção ELF
  `.relr.dyn` (relocações RELR) presente nas bibliotecas de sistema desta máquina — Arch é
  bleeding-edge e já compila com RELR habilitado por padrão, coisa que o Ubuntu 22.04 (o runner
  real do CI) não faz do mesmo jeito. Ou seja: é uma incompatibilidade específica de **buildar
  AppImage num host Arch**, não um problema do workflow, do `tauri.conf.json` ou do app — no
  runner `ubuntu-22.04` do GitHub Actions (ambiente padrão, testado por um sem-número de
  projetos Tauri) isso não deve reproduzir. Registrado aqui pra próxima sessão não perder tempo
  redescobrindo isso caso volte a testar build local de AppImage nesta máquina
- YAML do workflow validado com `ruby -ryaml` (não tinha `pyyaml`/`pip` disponível no python3
  do sistema)
- `project/ARCHITECTURE.md` (decisão de build/release registrada), `project/PHASE.md` (6.8
  concluída — Fase 6 inteira concluída), `project/OVERVIEW.md` (tabela de status corrigida:
  Fase 6 estava marcada `[ ] Pendente` há várias sessões, defasada desde que a fase começou)

**Próximo passo**: Fase 6 (App Desktop Nativo) está com todas as etapas concluídas. A
verificação completa do pipeline de release (legs Windows/macOS de verdade, e a criação de um
GitHub Release draft) só acontece empurrando uma tag `v*` pro remoto — não fiz isso
automaticamente por ser uma ação que afeta o repositório público (cria tag + dispara CI +
publica release draft); fica pro usuário decidir quando. Escolher a próxima fase a atacar (2 a
5, ainda todas pendentes, ou pular direto pra 7/8/9/10) é a decisão em aberto.

---

### 2026-08-04 — Sessão 20

- **Objetivo**: Etapa 6.7 — renderização de markdown nas mensagens do chat desktop.

**O que foi feito**:

- Adicionadas `react-markdown` (v10) + `remark-gfm` (tabelas, strikethrough, etc. do GFM) ao
  `desktop`. Decisão deliberada de **não** incluir syntax highlighting (ex. `rehype-highlight`) —
  6.7 pede renderização de markdown, não highlighting de código; blocos `pre`/`code` ficam com
  fonte monoespaçada e fundo próprio, sem parser de linguagem
- `MessageBubble.tsx`: trocado o `<p>` de texto puro por `<ReactMarkdown remarkPlugins={[remarkGfm]}>`.
  Componente customizado pro elemento `a` — intercepta o clique e abre o link no navegador padrão
  via `openUrl` do `@tauri-apps/plugin-opener` (`event.preventDefault()` + `openUrl(href)`) em vez
  de navegar a própria webview do app pra fora. A permissão `opener:default` já existia em
  `capabilities/default.json` desde o scaffold inicial, só não tinha uso real ainda no frontend
- `App.css`: `.message-bubble-content` deixou de ser um `<p>` com `white-space: pre-wrap` e virou
  um container com regras pra cada elemento markdown (`p`, `ul`/`ol`/`li`, `h1`-`h4`, `code`/`pre`,
  `blockquote`, `table`/`th`/`td`, `a`) — margens colapsadas no primeiro/último filho pra não sobrar
  espaço extra na bolha. Estilos de código/link **duplicados por variante** (`--user`/`--assistant`)
  porque a bolha do usuário é fundo sólido roxo com texto branco (link branco sublinhado, código com
  `rgba(255,255,255,0.16)`) enquanto a do assistente usa os tokens de tema existentes
  (`--color-surface-alt`, `--color-accent-dark`) — mesmo padrão dos outros componentes, que já
  segue `prefers-color-scheme` automaticamente por herdar as CSS custom properties
- Verificação real (não só `tsc`/build): como não dá pra digitar na janela Tauri nesse ambiente
  (Wayland/KDE, sem `xdotool`/`wtype` — limitação conhecida das sessões anteriores), rodei
  `npm run dev` (Vite puro, fora do Tauri) e dirigi um Chromium headless via Playwright
  (instalado temporariamente, desinstalado no final — não ficou como dependência do projeto) pra
  digitar uma mensagem markdown de verdade e tirar screenshot. Duas rodadas: (1) bolha do usuário
  com heading, bold/italic, inline code, lista, code block, blockquote e link — sem mock de IPC,
  só confirma que o `appendMessage` local (que já roda antes do `await invoke("send_message")`)
  renderiza certo; (2) `window.__TAURI_INTERNALS__.invoke` mockado pra simular uma resposta do
  assistente com tabela GFM, code block e link — bolha clara do assistente confirmada com bom
  contraste. Ambos os screenshots mostraram o roxo/tema intactos e nenhum elemento markdown
  quebrado. `cargo build/clippy --workspace` (sem mudança no lado Rust) e `npm run build` (tsc+vite)
  limpos
- `PHASE.md` atualizado (6.7 concluída)

**Próximo passo**: só falta 6.8 (build Linux/Windows/macOS) pra fechar a Fase 6 inteira. Vale
também o usuário confirmar visualmente a 6.7 rodando de verdade dentro do Tauri (não só via
Playwright fora dele) quando for prático.

---

### 2026-08-03 — Sessão 19

- **Objetivo**: Etapa 6.6 — histórico de conversas persistente no app desktop.

**O que foi feito**:

- Antes de codar, exploração confirmou que não existia nenhum precedente de persistência de
  histórico no projeto (nem o `warden-cli`, que só acumula um `Vec<Message>` em memória durante o
  loop REPL) — 6.6 era a primeira implementação disso. Decisão de formato/local registrada em
  `ARCHITECTURE.md`: JSON, um arquivo por conversa, em `~/.config/warden/conversations/<id>.json`
  — mesmo `dirs::config_dir()` já usado pro `config.toml` (dado opaco de app, ao contrário do
  vault markdown, que fica de propósito fora dessa pasta por ser humano-navegável)
- `warden-bootstrap`: novo `Conversation`/`ConversationMessage`/`ChatRole` (`Serialize`+
  `Deserialize`, `camelCase`, espelhando `desktop/src/types.ts` campo a campo — nenhum reshape na
  fronteira IPC); `default_conversations_dir()`, `save_conversation` (mkdir -p + overwrite
  completo do arquivo, mesmo padrão de `save_config`) e `list_conversations` (ordena por
  `updated_at` decrescente; diretório ausente = lista vazia, não erro, mesma convenção de
  `load_config_from_path` não-obrigatório; arquivo que falha o parse é **pulado**, não derruba a
  lista inteira — decisão deliberada pra um JSON corrompido não sumir com todo o histórico da
  sidebar). 5 testes novos (12 no total no crate)
- `desktop/src-tauri`: dois comandos novos, `list_conversations`/`save_conversation`, wrappers
  finos sobre as funções do `warden-bootstrap` (importadas com alias — `read_conversations`/
  `write_conversation` — pra não colidir de nome com os comandos Tauri). Comentário do `ChatTurn`
  atualizado (já não é mais verdade que "a fronteira do frontend é quem possui o histórico");
  `ChatTurn` continua existindo separado de `ConversationMessage` de propósito — `send_message`
  só precisa de role+content, não do id/timestamps que um registro persistido carrega
- Frontend: `App.tsx` ganhou `useEffect` que chama `list_conversations` no mount (populando o
  estado a partir do disco em vez de começar sempre vazio) e `appendMessage` passou a computar o
  objeto `Conversation` atualizado explicitamente (antes só existia dentro do updater do
  `setConversations`) pra poder chamar `save_conversation` com ele — fire-and-forget, erro só vai
  pro console, não bloqueia o envio da mensagem (persistência falhando não deveria quebrar o chat
  da sessão atual)
- Verificação: `cargo build/test/clippy --workspace` limpos (30 testes: 12 warden-bootstrap + 5
  CLI + 10 core + 3 pipeline); `npm run build` (tsc+vite) limpo; `npm run tauri dev` rodou ~40s+
  sem crash nem erro no log após compilar (mesma limitação de sempre pra testar de verdade:
  sem `xdotool`/`wtype` nesse Wayland pra simular clique/digitação). Fica pro usuário confirmar
  visualmente: mandar mensagens, fechar o app, reabrir, e ver a conversa na sidebar com o
  histórico intacto
- `PHASE.md` (6.6 concluída), `ARCHITECTURE.md` (decisão de formato/local do histórico registrada)

**Próximo passo**: Fase 6 só tem 6.7 (renderização de markdown nas mensagens) e 6.8 (build
Linux/Windows/macOS) restando. Vale o usuário testar a 6.6 de verdade (e a 6.5, ainda pendente de
confirmação da Sessão 18) antes de seguir.

---

### 2026-08-02 — Sessão 18

- **Objetivo**: Etapa 6.5 — configuração visual (modelo, API keys, vault path) no app desktop.

**O que foi feito**:

- Antes de codar, duas explorações em paralelo (sistema de config em `warden-bootstrap` e
  estrutura do frontend desktop) revelaram três lacunas reais que a 6.5 precisava resolver, não só
  desenhar uma tela: **não existia função pra escrever** o TOML de volta (`FileConfig`/`ApiKeys`
  só tinham `Deserialize`), **não existia reload** do orchestrator (montado uma única vez no
  `run()`, `AppState` sem `Mutex`), e o `Overrides` não tinha campo pra chaves de API
- Perguntado ao usuário duas decisões de escopo antes de fechar o plano: (1) as chaves de API
  aparecem na tela mascaradas com um toggle de "olhinho" pra revelar, editáveis diretamente — não
  só um booleano "configurada/não configurada"; (2) o campo de vault path ganha seletor nativo de
  pasta (`tauri-plugin-dialog`), não só texto. A decisão (1) simplificou bastante o backend: como
  o formulário sempre vem pré-preenchido com os valores reais, salvar virou um **overwrite
  completo** do arquivo de config, sem precisar de lógica de merge parcial ("None = manter, Some =
  sobrescrever") que tinha sido o desenho inicial
- `warden-bootstrap`: `Provider`/`FileConfig`/`ApiKeys` ganharam `Serialize`; nova
  `save_config(path, &FileConfig)` (cria diretório pai se preciso, escreve TOML bonito); extraído
  `default_model_for(Provider) -> &str` do `bootstrap()` (refactor comportamento-preservando, usado
  agora tanto pelo `bootstrap` quanto pelo `get_settings` do desktop pra não duplicar os literais
  `"gemini-2.5-flash"`/`"gpt-4o-mini"` em TypeScript). 2 testes novos (7 no total no crate):
  round-trip completo `save_config` → `load_config_from_path`, e criação do diretório pai ausente
- `warden-core`: `Orchestrator` ganhou `#[derive(Clone)]` — de graça, já que todo campo é `Arc`
  (ou `Vec<Arc<_>>`). É o que permite `send_message` clonar o orchestrator de dentro do mutex e
  soltar o lock antes do `.await`, em vez de segurar um `MutexGuard` através de um ponto de espera
- `desktop/src-tauri`: `AppState.orchestrator` virou `Mutex<Result<Orchestrator, String>>`.
  Dois comandos novos: `get_settings` (lê o TOML atual do disco, devolve provider/model/vault_path/
  as três chaves em texto puro — string vazia = "não definido", mesma convenção do `ChatTurn` de
  manter a fronteira IPC em strings simples) e `save_settings` (monta um `FileConfig` completo a
  partir do formulário, `save_config`, chama `bootstrap()` de novo e troca o resultado dentro do
  mutex — live-reload sem reiniciar o app). Adicionado `tauri-plugin-dialog` (Cargo.toml +
  package.json + permissão `dialog:default` em `capabilities/default.json`) pro seletor nativo de
  pasta do vault path
- Frontend: `types.ts` ganhou `Settings`/`ModelProvider`; `SettingsView.tsx` novo (form completo:
  provider, model com placeholder do default, vault path com botão "Browse…", três campos de API
  key com toggle 👁/🙈 de revelar); `Sidebar.tsx` ganhou botão de engrenagem no header
  (`onOpenSettings`); `App.tsx` ganhou estado `view: "chat" | "settings"` — sidebar sempre visível,
  troca só o painel direito; `onNewConversation`/`onSelectConversation` também voltam pra `"chat"`
  se a Settings estiver aberta. CSS só aditivo em `App.css`, reaproveitando os tokens `--color-*` e
  os padrões visuais já existentes (inclusive `color-mix`, que já era usado no banner de erro do
  chat)
- Verificação: `cargo build/test/clippy --workspace` limpos (27 testes); `npm run build` (tsc+vite)
  limpo; `npm run tauri dev` rodou por ~55s sem crash nem erro no log (confirma que o Mutex, o
  plugin novo e a permissão `dialog:default` não quebraram o startup) — mas não deu pra clicar/
  digitar de verdade (mesma limitação de sempre: sem `xdotool`/`wtype` nesse Wayland). Fica pro
  usuário confirmar visualmente o fluxo completo: abrir Settings, revelar/editar uma chave, usar o
  seletor de pasta, salvar, confirmar que persiste após reiniciar o app, e que uma mensagem enviada
  logo depois de salvar já usa o provider novo sem precisar reiniciar
- `PHASE.md` (6.5 concluída)

**Próximo passo**: Fase 6 só tem 6.6 (histórico de conversas — hoje as conversas somem ao fechar o
app, só vivem no estado do React), 6.7 (renderização de markdown nas mensagens) e 6.8 (build
Linux/Windows/macOS) restando. Ou o usuário testar 6.5 de verdade antes de seguir.

---

### 2026-08-02 — Sessão 17

- **Objetivo**: Etapa 6.4 — canal nativo (chat direto no app).

**O que foi feito**:

- Antes de mexer no desktop, encontrado o real motivo pelo qual 6.4 não estava de fato pronta
  apesar da 6.3: `Orchestrator::handle_message` era completamente stateless entre chamadas — cada
  turno virava uma conversa nova pro modelo, só com o vault-context injetado e a mensagem atual.
  O histórico visível na UI do desktop (array `messages` por conversa) nunca era enviado de volta
  pro modelo. Isso valia tanto pro desktop quanto pro REPL do `warden-cli`
- `Orchestrator::handle_message` ganhou um parâmetro `history: &[Message]` (turnos anteriores,
  mais antigo primeiro), inserido entre o system message de contexto do vault e a mensagem nova do
  usuário. `&[]` continua válido pra conversa nova ou tarefa de sub-agente avulsa (`DelegateTool`
  passa `&[]` deliberadamente — sub-agente não deve ver a conversa do pai, isso já era intencional)
- `warden-cli`: o loop REPL agora acumula um `Vec<Message>` e alimenta ele de volta a cada chamada
- Desktop: comando Tauri `send_message` ganhou parâmetro `history: Vec<ChatTurn>` (struct local com
  `Deserialize`, espelha o `ChatRole`/`ChatMessage` do frontend em `types.ts`) convertido pra
  `Vec<Message>`. `App.tsx` monta esse histórico a partir de `activeConversation.messages` (antes
  de anexar a nova mensagem do usuário) e manda junto no `invoke`
- Novo teste de integração em `warden-core/tests/pipeline.rs`
  (`prior_turns_are_sent_to_the_model_on_the_next_call`) que prova via `ScriptedModel` que turnos
  anteriores (user + assistant) chegam de fato na lista de mensagens da chamada seguinte — sem
  esse teste o bug de "modelo não lembra da conversa" não seria pego por nada
- Verificação: `cargo build/test/clippy --workspace` limpos (25 testes, incluindo o novo);
  `npm run build` (tsc+vite) limpo. Não testado o envio de mensagem de verdade no app (mesma
  limitação da Sessão 16 — sem `GEMINI_API_KEY` no ambiente e sem `xdotool`/`wtype` nesse Wayland
  pra simular digitação); fica pro usuário confirmar visualmente que o modelo agora lembra de
  turnos anteriores dentro da mesma conversa
- `PHASE.md` (6.4 concluída)

**Próximo passo**: 6.5 — configuração visual (tela de settings pra API keys/provider/vault) segue
como próximo item natural da Fase 6; ou o usuário testar 6.4 de verdade antes de seguir (mandar
duas mensagens em sequência e confirmar que a segunda referencia a primeira).

---

### 2026-08-02 — Sessão 7

- **Objetivo**: Mapear P14 (o que realmente exige servidor) a pedido do usuário — sem código nesta sessão.

**O que foi feito**:

- Passei por todas as features já registradas (Fases 1-10 + P8-P16) e classifiquei cada uma em três
  baldes: não precisa de servidor, precisa só de "algo sempre ligado" (uptime, não topologia
  servidor↔cliente), ou precisa de fato do node primário/servidor
- Conclusão registrada em `ARCHITECTURE.md` ("Mapa de dependência de servidor"): só a **Fase 9**
  (execução remota de tool em outro device, pareamento multi-device) exige de fato o papel servidor.
  Canais Telegram/WhatsApp só precisam de um processo sempre ligado, não de topologia estrela. A
  "Warden API" (P12) e o Warden como server MCP (P11b) só entram nessa categoria se chamados de fora
  do device onde o Warden roda — localmente, não precisam de nada
- Reforcei o reframe: "servidor" no Warden nunca é infra de terceiro — é sempre uma das próprias
  máquinas do usuário designada como "a que fica ligada". Não existe cenário que exija algo que o
  usuário não já controle
- P14 movida pra "Resolvidas" em `PENDING.md`, com nota de que pode ser revista quando a Fase 9 for
  implementada de fato e detalhes concretos aparecerem

**Próximo passo**: retomar a Fase 1 (1.4/1.5 — Vault injetando contexto no orchestrator) quando o
usuário quiser voltar à implementação.

---

### 2026-08-02 — Sessão 6

- **Objetivo**: Refinar P11/P12 com base em feedback do usuário sobre as ideias registradas na Sessão 5, e registrar duas ideias novas — sem código nesta sessão.

**O que foi feito**:

- Dei minha opinião sobre P9–P13: sinalizei risco de segurança em P11 server-side (expor vault via
  MCP pra terceiros sem escopo granular), tensão de identidade em P12 (agente pessoal self-hosted vs
  virar plataforma SaaS multi-tenant), e sugeri não reinventar indexador de arquivo/app em P9
  (reaproveitar `mdfind`/`Everything`/`plocate` em vez de indexação própria)
- Usuário esclareceu P11: exemplo concreto é Warden conectar via MCP com o **Anchor** e criar um
  valuation por conta do usuário; e a parte mais ambiciosa é **P15** — conectores MCP genéricos,
  não ficar restrito a integrações que já têm servidor MCP pronto no mercado
- Usuário esclareceu P12: a chave de API é criada dentro do próprio app (não é serviço à parte),
  totalmente opcional, roda no "app host" que já está no device do usuário
- **Refinamento arquitetural importante**: a topologia estrela (`ARCHITECTURE.md`) não significa que
  o servidor é sempre necessário — um único node deve funcionar 100% standalone. Servidor só entra
  pro que exige coordenação entre múltiplos devices (ex. gerenciamento multi-máquina, Fase 9).
  Não está confirmado se a "Warden API" (P12) ou hospedar MCP acessível de fora exigem servidor —
  registrado como **P14**, decisão explicitamente em aberto
- Nova ideia registrada: **P16** — sistema de "Skills" configuráveis pela UX (não só código), no
  espírito das Skills do Claude, ainda sem arquitetura definida
- Atualizados `ARCHITECTURE.md` (nota "servidor é opcional"), `PENDING.md` (P11/P12 refinados,
  P14/P15/P16 novos), `ROADMAP.md` (seções de MCP e Warden API reescritas, seção nova de Skills)

**Próximo passo**: retomar a Fase 1 (1.4/1.5 — Vault injetando contexto no orchestrator) quando o
usuário quiser voltar à implementação.

---

### 2026-08-02 — Sessão 5

- **Objetivo**: Registrar novas ideias de produto do usuário — sem código nesta sessão.

**O que foi feito**:

- Registradas cinco ideias novas em `ROADMAP.md`/`PENDING.md` (P9–P13):
  - **P9** — App "Copilot" leve rodando em segundo plano no SO (Linux/Windows/Mac), tipo
    Spotlight/PowerToys Run, busca apps/arquivos/pastas, atalho global, opt-in
  - **P10** — Dashboard de consumo de tokens e gerenciamento de chaves de API, com custo
    estimado por provedor/modelo (complementa P4, que é a parte de backend)
  - **P11** — Tela de gerenciamento de integrações MCP, nos dois sentidos: Warden como
    client MCP (consumindo servers externos) e como server MCP (expondo tools/vault pra terceiros)
  - **P12** — "Warden API": chave de API do próprio Warden, não do provedor por trás —
    abstrai o modelo escolhido e já injeta o contexto do vault do usuário
  - **P13** — Integração via MCP com o ecossistema do usuário: Practice Valuation
    (rebrand pra Anchor) e TruthID
- Adicionada seção "Ecossistema" em `CONTEXT.md` explicando que o Warden é parte de um
  ecossistema open-source descentralizado maior (TruthID + Practice Valuation/Anchor + Warden)

**Próximo passo**: retomar a Fase 1 (1.4/1.5 — Vault injetando contexto no orchestrator) quando o
usuário quiser voltar à implementação.

---

### 2026-08-02 — Sessão 4

- **Objetivo**: Adicionar `GeminiProvider` e priorizá-lo como provedor padrão (API gratuita).

**O que foi feito**:

- Implementado `GeminiProvider` (`crates/warden-core/src/model/gemini.rs`) — segunda implementação de
  `ModelProvider`, chama `generateContent` da API do Gemini. Schema bem diferente do OpenAI: roles
  `user`/`model` (não `assistant`), mensagem de sistema vai em `system_instruction` separado, tools em
  formato `function_declarations` agrupado (não um objeto por tool)
- `warden-cli` ganhou flag `--provider` (`gemini` | `openai`, default `gemini`) e `--model` (opcional,
  default depende do provider: `gemini-2.5-flash` ou `gpt-4o-mini`). Key lida de `GEMINI_API_KEY` ou
  `OPENAI_API_KEY` conforme o provider escolhido
- Testado: erro claro sem key, erro real da API do Gemini com key inválida (confirma que a request
  chega certa), `--provider openai` continua funcionando
- **Atenção**: nome do modelo Gemini default (`gemini-2.5-flash`) é o mais recente conhecido até o
  cutoff de conhecimento do Claude (jan/2026) — vale confirmar em aistudio.google.com se ainda é o
  correto/gratuito

**Próximo passo**: 1.4/1.5 — plugar o `Vault` no orchestrator pra injetar contexto relevante no prompt.

---

### 2026-08-02 — Sessão 8

- **Objetivo**: Etapas 1.4 e 1.5 — vault markdown local completo + busca de contexto injetada no prompt.

**O que foi feito**:

- `Vault::new` agora cria a pasta raiz automaticamente (`create_dir_all`) se não existir, em vez de
  falhar silenciosamente no primeiro `read`/`list`
- Adicionado `Vault::list_files()` — varre a raiz recursivamente e retorna só arquivos `.md`
  (paths relativos à raiz)
- Adicionado `Vault::search(query, max_hits)` — grep simples: quebra a query em palavras (≥3
  caracteres), busca substring case-insensitive linha a linha em todo `.md` do vault, retorna
  `SearchHit { path, line_number, line }`. Resolve a P5 pra v1 (grep simples; embedding fica pra
  Fase 4, ver P6)
- `Orchestrator::handle_message` agora chama `vault.search` com o texto do usuário como query antes
  de montar as mensagens; se houver hits, injeta uma `Message { role: System }` com o contexto
  encontrado (path:linha + conteúdo) antes da mensagem do usuário. Ambos providers (`OpenAiProvider`,
  `GeminiProvider`) já tratavam `Role::System` corretamente, não precisou mexer neles
- Testes unitários em `memory/mod.rs` (roundtrip de write/read, listagem só de `.md` em subpastas,
  busca case-insensitive + respeito ao limite `max_hits`) — `cargo test --workspace` passa (3 testes)
- `cargo build --workspace` limpo
- `PHASE.md` atualizado (1.4 e 1.5 concluídas), `PENDING.md` (P5 resolvida)

**Próximo passo**: 1.6 — trait `Tool` já existe (`crates/warden-core/src/tool/mod.rs`), falta a
primeira implementação concreta (`read_file`/`write_file`, provavelmente sobre o próprio `Vault`) e
registrá-la no `Orchestrator` via `register_tool`.

---

### 2026-08-02 — Sessão 9

- **Objetivo**: Etapa 1.6 — trait `Tool` (já existia) + primeiras tools concretas (`read_file`,
  `write_file`) registradas no orchestrator.

**O que foi feito**:

- Percebido que só criar as structs de tool não bastava: sem um loop de tool-calling, o modelo nunca
  teria como efetivamente chamá-las. Implementado o ciclo completo:
  - `model::Message` ganhou `tool_calls: Vec<ToolCall>` e `tool_call_id`/`tool_name` (pra respostas de
    tool), com construtores (`Message::system/user/assistant/assistant_tool_calls/tool_result`) no
    lugar de literais de struct espalhados
  - `model::Response` ganhou `tool_calls: Vec<ToolCall>`
  - `OpenAiProvider`: serializa `tool_calls` do assistant e mensagens `role: "tool"` com
    `tool_call_id`; faz parse de `tool_calls` da resposta (`function.arguments` vem como string JSON,
    parseado pra `Value`)
  - `GeminiProvider`: schema bem diferente — `functionCall` (nos `parts` do `model`) e
    `functionResponse` (role `function`, chaveado por `name` já que Gemini não devolve um id real;
    geramos um `call_{i}` sintético só pra uso interno). `Message::tool_name` existe justamente pra
    isso, já que Gemini não usa `tool_call_id`
  - `Orchestrator::handle_message` agora roda um loop (cap de `MAX_TOOL_ITERATIONS = 8`): chama o
    modelo, se vier `tool_calls` executa cada uma via `run_tool` (procura a tool registrada pelo nome),
    anexa o resultado como `Message::tool_result` e chama de novo; se vier só `content`, retorna. Erro
    claro se estourar o limite de iterações (evita loop infinito de um modelo "preso")
- Implementadas `ReadFileTool`/`WriteFileTool` (`crates/warden-core/src/tool/file_tools.rs`), ambas
  sobre `Arc<Vault>` — schema JSON simples (`path` e `path`+`content`)
- `Orchestrator::new` e `vault()` passaram a usar `Arc<Vault>` (antes era owned), pra permitir o mesmo
  vault ser compartilhado entre o orchestrator e as tools sem clonar o conteúdo
- `warden-cli`/`main.rs` registra as duas tools no orchestrator logo após criá-lo
- Testes: `warden-core` ganhou dev-dependency `tokio` (pra `#[tokio::test]`). Cobertura nova —
  round-trip `write_file`→`read_file`, erro claro faltando `path`, um `MockModel` que simula um
  primeiro turno pedindo a tool `echo` e um segundo turno respondendo `"done"` (valida que o resultado
  da tool chega de volta como `Message` de role `Tool`), e um teste de que o orchestrator desiste após
  `MAX_TOOL_ITERATIONS` em vez de rodar pra sempre. `cargo test --workspace`: 7 testes, todos passando.
  `cargo clippy --workspace --all-targets`: limpo
- `PHASE.md` atualizado (1.6 concluída)

**Próximo passo**: 1.7 — tool `web_search` (pesquisa na internet via API). Depois, 1.8 (sub-agente
leve) e 1.9 (testes de integração ponta a ponta do pipeline via CLI).

---

### 2026-08-02 — Sessão 10

- **Objetivo**: Etapa 1.7 — tool `web_search` (pesquisa na internet via API).

**O que foi feito**:

- Decisão de qual API de busca usar não estava registrada em nenhum lugar — perguntei ao usuário
  entre Tavily, Brave Search API e Google Custom Search JSON API. Escolhido **Tavily**: feita
  especificamente pra tool use de agentes LLM (resultados já vêm como snippets curtos, não HTML cru),
  free tier de 1.000 buscas/mês sem exigir cartão de crédito no cadastro
- Implementado `WebSearchTool` (`crates/warden-core/src/tool/web_search.rs`) — `POST
  https://api.tavily.com/search` com `{api_key, query, max_results: 5}`, retorna `{results: [{title,
  url, content}]}`
- `warden-cli`/`main.rs`: tool só é registrada se `TAVILY_API_KEY` estiver setada — se não estiver,
  o Warden roda normalmente sem ela (mensagem de aviso no start, não erro fatal). Segue o mesmo
  espírito de "servidor opcional"/degradação graciosa já registrado em P14: nem toda capability exige
  todo pré-requisito configurado
- Teste unitário cobrindo validação do argumento `query` obrigatório (sem chamar a API de verdade —
  não há mock de HTTP no projeto ainda, então só o que não depende de rede é testado aqui)
- Testado manualmente: `cargo run` com `GEMINI_API_KEY` fake e sem `TAVILY_API_KEY` mostra o aviso e
  sobe normal
- `cargo build`/`test`/`clippy --all-targets` limpos (8 testes)
- `PHASE.md` atualizado (1.7 concluída)

**Próximo passo**: 1.8 — sub-agente leve (delegar tarefa escopada pra outro modelo/contexto). Depois,
1.9 (testes de integração ponta a ponta) e 1.10 (config via YAML/TOML).

---

### 2026-08-02 — Sessão 11

- **Objetivo**: Etapa 1.8 — sub-agente leve: delegar tarefa escopada pra outro modelo/contexto.

**O que foi feito**:

- Planejado com um agente de arquitetura antes de implementar, pra validar a abordagem (evitar
  duplicar lógica, garantir `Send + Sync`, revisar o schema da tool). Design confirmado: sub-agente
  leve = mais uma `Tool`, não um mecanismo novo
- Implementado `DelegateTool` (`crates/warden-core/src/tool/delegate.rs`), tool `delegate_task`:
  internamente possui um `Orchestrator` completo (mesmo `model`, mesmo `vault`, subconjunto de tools
  escolhido pelo chamador) e delega pra `Orchestrator::handle_message` — reaproveita 100% do loop de
  tool-calling já existente (injeção de contexto do vault, cap de iterações, etc.) sem duplicar lógica
- Prevenção estrutural de recursão: o sub-`Orchestrator` passado pro `DelegateTool` nunca recebe
  outro `DelegateTool` registrado (é montado a partir de um conjunto de tools "base", sem o próprio
  `delegate_task`), então não existe caminho de código pra um sub-agente lançar outro sub-agente —
  consistente com a decisão em `ARCHITECTURE.md` de que sub-agentes autônomos/recursivos ficam fora
  do escopo v1
- `warden-cli`/`main.rs` reestruturado: as tools "base" (`read_file`, `write_file`, `web_search` se
  `TAVILY_API_KEY` estiver setada) agora são construídas uma vez como `Vec<Arc<dyn Tool>>` e
  registradas em dois orchestrators — um "sub" (usado só pra montar o `DelegateTool`) e o principal,
  que também ganha o `delegate_task` — evita duplicar construção de `Vault`/cliente HTTP
  (`Arc<dyn Tool>` é barato de clonar/registrar em múltiplos orchestrators)
- Confirmado que `Orchestrator` é `Send + Sync` automaticamente (todos os campos —
  `Arc<dyn ModelProvider>`, `Arc<Vault>`, `Vec<Arc<dyn Tool>>` — já são `Send + Sync`), então
  guardá-lo direto (sem `Arc` extra) dentro de `DelegateTool` e expor esse `DelegateTool` como
  `Arc<dyn Tool>` funciona sem `unsafe`
- Testes unitários em `tool/delegate.rs`: erro claro quando falta o argumento `task`; round-trip
  completo com um `ModelProvider` mock de resposta fixa (sem tool_calls), confirmando que
  `DelegateTool::call` repassa a tarefa pro sub-orchestrator e devolve `{ "result": ... }`
- `cargo build`/`test`/`clippy --all-targets` limpos (10 testes)
- `PHASE.md` atualizado (1.8 concluída), `ARCHITECTURE.md` (nota do `DelegateTool` na seção de
  sub-agentes)

**Próximo passo**: 1.9 — testes de integração do pipeline completo (CLI). Depois, 1.10 — config via
YAML/TOML (modelo, API keys, vault path).

---

### 2026-08-02 — Sessão 12

- **Objetivo**: Etapa 1.9 — testes de integração do pipeline completo (CLI).

**O que foi feito**:

- Até aqui só existiam testes unitários dentro de cada módulo (`memory`, `tool::*`,
  `orchestrator`), cada um testando uma peça isolada com mocks locais. Faltava algo que provasse
  que a fiação entre as peças — exatamente como o `main.rs` monta (`Vault` + tools compartilhadas
  entre dois `Orchestrator`s + `DelegateTool`) — realmente funciona junta
- Criado `crates/warden-core/tests/pipeline.rs` (teste de integração de verdade, crate separada
  que só enxerga a API pública do `warden-core`, sem chamada de rede real):
  - `vault_context_and_read_file_tool_round_trip` — escreve uma nota no vault, manda uma pergunta
    cujas palavras batem com a nota, e um `ScriptedModel` (mock genérico com uma closure por
    chamada, generaliza o padrão `MockModel`/`AtomicUsize` já usado em outros testes) confirma que
    o contexto do vault chega como `Message::system` *e* que pedir a tool `read_file` e receber o
    resultado de volta funciona ponta a ponta
  - `delegate_task_round_trip_through_full_wiring` — monta a pilha exatamente como o `main.rs`
    (tools "base" compartilhadas entre `sub_orchestrator` e o orchestrator principal via `Arc<dyn
    Tool>`, `DelegateTool` só no principal) e confirma que o orchestrator de fora consegue de fato
    despachar a tool `delegate_task` pelo nome e receber o resultado do sub-agente de volta — isso
    não estava coberto antes (os testes de `delegate.rs` só chamavam `DelegateTool::call`
    diretamente, nunca através do loop de tool-calling do `Orchestrator`)
- Criado `crates/warden-cli/tests/cli.rs` — testes de processo de verdade, via
  `std::process::Command` + `env!("CARGO_BIN_EXE_warden")` (sem precisar de crate extra tipo
  `assert_cmd`), com `env_clear()` pra não vazar env vars do host: erro claro sem
  `GEMINI_API_KEY`/`OPENAI_API_KEY` (exit code != 0, mensagem certa no stderr); startup limpo com
  key fake + `exit` via stdin (banner no stdout, aviso de `TAVILY_API_KEY` no stderr, pasta do
  vault criada no disco). Todos os cenários falham antes de qualquer chamada de rede, então não
  precisam de API key real nem de mock de HTTP
- `warden-core/Cargo.toml`: nada novo em `[dependencies]` precisou virar dev-dependency —
  `async-trait`/`serde_json` já eram dependências normais (disponíveis em testes de integração por
  padrão); só `tokio` já estava como dev-dependency desde a sessão 9
- `cargo build`/`test`/`clippy --all-targets` limpos — 15 testes no total (10 unitários + 2 de
  pipeline + 3 de processo do CLI)
- `PHASE.md` atualizado (1.9 concluída)

**Próximo passo**: 1.10 — configuração via arquivo YAML/TOML (modelo, API keys, vault path). Isso
fecha a Fase 1; depois entra a Fase 2 (canal Telegram).

---

### 2026-08-02 — Sessão 13

- **Objetivo**: Etapa 1.10 — configuração via arquivo YAML/TOML (modelo, API keys, vault path).
  Fecha a Fase 1 inteira.

**O que foi feito**:

- Decidido **TOML** em vez de YAML (não pedi confirmação ao usuário pra essa — é decisão técnica
  contida, sem tradeoff externo tipo assinatura de serviço, então segui o mesmo padrão de P2):
  convenção do próprio ecossistema Rust (mesmo formato do `Cargo.toml`), sem ambiguidades clássicas
  de parsing do YAML, crate `toml` madura e serde-native. Registrado em `ARCHITECTURE.md`
- Localização do arquivo: diretório de config do SO via crate `dirs`
  (`~/.config/warden/config.toml` no Linux, equivalente no Windows/macOS), com override via
  `--config <path>`
- `warden-cli`/`main.rs` reestruturado com precedência clara: **flag de CLI > variável de ambiente
  (só pra API keys) > arquivo de config > default embutido**. Pra isso, os campos do `Cli` (clap)
  que tinham `default_value`/`default_value_t` viraram `Option<T>` — sem isso não dava pra
  distinguir "usuário não passou a flag" de "usuário passou o valor default explicitamente"
- `FileConfig { provider, model, vault_path, api_keys: ApiKeys { gemini, openai, tavily } }`,
  tudo opcional, com `#[serde(deny_unknown_fields)]` (typo no config agora vira erro claro em vez de
  ser silenciosamente ignorado). `Provider` (o enum já existente de `--provider`) ganhou
  `#[derive(Deserialize)]` além do `ValueEnum` do clap, com `rename_all = "lowercase"` pra bater com
  o mesmo texto que já era aceito via CLI (`gemini`/`openai`)
- Semântica de "arquivo ausente" diferenciada por intenção: se o usuário passou `--config` e o
  arquivo não existe, é erro claro (ele pediu aquele arquivo especificamente); se é o caminho
  default do SO e não existe, cai silenciosamente pra config vazia (a maioria dos usuários ainda não
  vai ter criado um) — mesmo espírito de degradação graciosa já usado em P14/`TAVILY_API_KEY`.
  Extraído em duas funções (`load_config` fino chamando `load_config_from_path` com a lógica pura)
  justamente pra dar pra testar essa distinção sem depender do `dirs::config_dir()` real
- `resolve_secret(from_env, from_file)` centraliza a precedência de API keys (env var vence o
  arquivo, pra dar pra sobrescrever uma key salva só naquela execução sem editar o arquivo)
- Novas dependências (`Cargo.toml` do workspace): `toml` (parser) e `dirs` (diretório de config
  cross-platform) — ambas só em `warden-cli`, `warden-core` continua sem saber nada sobre arquivo
  de config (isso é decisão de camada de CLI/canal, não do core)
- Testes: 5 unitários novos em `main.rs` (parse de config válido, path não-obrigatório ausente ⇒
  config vazia, path obrigatório ausente ⇒ erro, TOML malformado ⇒ erro claro, precedência de
  `resolve_secret`) + 2 testes de processo novos em `tests/cli.rs` (sobe usando key e vault_path só
  do arquivo de config, sem nenhuma env var; `--config` apontando pra arquivo inexistente falha com
  mensagem clara)
- `cargo build`/`test`/`clippy --all-targets` limpos — 22 testes no total
- `PHASE.md` (1.10 concluída — **Fase 1 completa**), `OVERVIEW.md` (Fase 1 marcada como concluída no
  status geral), `ARCHITECTURE.md` (decisão TOML + localização + precedência)

**Próximo passo**: Fase 2 — Canal Telegram (2.1: setup do bot, token, webhook/polling).

---

### 2026-08-02 — Sessão 14

- **Objetivo**: Reordenar prioridades (usuário quer o app desktop antes de Telegram/WhatsApp) e
  implementar a etapa 6.1 — setup Tauri + React + TypeScript.

**O que foi feito**:

- Usuário pediu pra pular a ordem numérica das fases e ir direto pro app desktop, priorizando ter
  uma interface de chat de verdade em vez dos canais de mensageria. Validei que é seguro
  tecnicamente (app desktop só faz IPC local, não depende da topologia servidor↔cliente da Fase 9 —
  já mapeado em P14) e registrei a reordenação em `ROADMAP.md` (nota datada, números das fases em
  `PHASE.md` não mudaram, só a ordem de execução)
- Antes de implementar, usei um agente Explore pra levantar a stack Tauri real do TruthID
  (`/home/masterlxz/Documents/workspace/truthid/desktop`), já que o usuário confirmou "mesma
  stack": Tauri 2, React 19, TypeScript ~5.8, Vite 7, npm, sem router nem lib de state management
  (só Context), CSS puro sem Tailwind. TruthID não tem nenhuma UI de chat pra reaproveitar —
  território novo
- Scaffold gerado via `create-tauri-app` (`npx --yes create-tauri-app@latest desktop -m npm -t
  react-ts --identifier com.warden.desktop -y`) — confirma exatamente a stack do TruthID.
  `desktop/` na raiz do repo, irmã de `crates/`, não dentro dela (não é lib Rust compartilhável)
- Rebranding mínimo: `productName`/`windows[0].title` em `tauri.conf.json` → "Warden" (resto do
  config fica no default do template por enquanto — CSP/capabilities não valem endurecer ainda,
  não tem conteúdo remoto carregado)
- `desktop/src-tauri` entrou no workspace Cargo raiz (`Cargo.toml` → `members`), com
  `version.workspace = true`/`edition.workspace = true` pra consistência com `warden-core`/
  `warden-cli`. **Sem** dependência em `warden-core` ainda — isso é explicitamente 6.3 (IPC), não
  6.1; entrar no workspace agora é só estrutural, garante que `cargo build/test/clippy --workspace`
  já cobre esse crate a partir de agora
- Corrigido `.gitignore`: a seção "# Tauri" antiga (`src-tauri/target/`, `src-tauri/icons/**`) tinha
  sido escrita antecipando um `src-tauri/` na raiz do repo — como o app real ficou em
  `desktop/src-tauri/`, essas regras (ancoradas por terem `/` no meio) nunca bateriam de verdade.
  Removidas: o scaffold já gera seus próprios `.gitignore` aninhados (`desktop/.gitignore`,
  `desktop/src-tauri/.gitignore`) que cobrem `target/`/`node_modules/`/`dist/` corretamente pro
  caminho real — e de propósito **não** ignoro os ícones (devem ser commitados, igual o TruthID
  faz, senão o build de bundle quebra em quem clonar o repo sem eles)
- Diferente do `Cargo.lock` (ignorado no repo), decidi commitar `desktop/package-lock.json` —
  mesmo precedente do TruthID, e o ecossistema npm se beneficia mais de lockfile fixado
  (resolução de transitivas mais volátil que a do Cargo)
- Verificação: `cargo build --workspace` (~4min na primeira vez, todo o GTK/webkit2gtk do Tauri
  compilando do zero) e `cargo clippy --workspace --all-targets` limpos; `npm install && npm run
  build` (tsc + vite build) sem erro; `npm run tauri dev` rodado de verdade em background — janela
  "Warden" subiu (processo `target/debug/desktop` confirmado rodando, log de compilação sem erro).
  Tela do ambiente estava bloqueada, então não deu pra tirar screenshot da janela de verdade — usuário
  pode conferir visualmente rodando `npm run tauri dev` ele mesmo. `cargo test --workspace`
  confirma que os 17 testes já existentes continuam passando com o novo membro no workspace
- Usuário pediu, en passant: identidade visual parecida com o TruthID mas **roxo vibrante** em vez
  de azul — registrado em `PHASE.md` (Fase 6) pra valer a partir da 6.2, já que a 6.1 ainda é só
  boilerplate padrão do template, sem branding nenhum
- `PHASE.md` (6.1 concluída + nota de identidade visual), `ROADMAP.md` (reordenação de prioridade)

**Próximo passo**: 6.2 — shell do app (sidebar de conversas, área de chat), já com a identidade
visual roxa em mente.

---

### 2026-08-02 — Sessão 15

- **Objetivo**: Etapa 6.2 — shell do app: sidebar de conversas + área de chat, tudo com estado
  local do React (sem IPC/backend ainda — isso é a 6.3).

**O que foi feito**:

- Planejado com um agente de arquitetura antes de implementar (leu o estado real dos arquivos do
  scaffold da 6.1 e o padrão do TruthID). Decisões principais validadas: sem lib de state
  management (só `useState` em `App.tsx`, igual TruthID), componentes flat em
  `desktop/src/components/` sem subpastas por feature, tipos locais próprios em vez de espelhar o
  `Role`/`Message` completo do `warden-core` (só `user`/`assistant` aparecem na UI — `System`/
  `Tool` são detalhe interno)
- Criados `desktop/src/types.ts` (`ChatRole`, `ChatMessage`, `Conversation`) e os componentes
  `Sidebar.tsx`, `ChatArea.tsx`, `MessageBubble.tsx` (extraído já pensando na 6.7 — renderização de
  markdown vai mexer só nesse arquivo), `MessageInput.tsx` (Enter envia, Shift+Enter quebra linha,
  autofoco, desabilitado com input vazio)
- Decisão de estado que vale registrar: `activeConversationId === null` é estado normal
  ("composer pronto, nada enviado ainda"), não erro — dá pra mandar a primeira mensagem sem clicar
  em "nova conversa" antes. Clicar em "nova conversa" só zera o id ativo, não cria entrada vazia no
  array — toda `Conversation` sempre tem ≥1 mensagem, então a UI nunca precisa tratar "conversa
  existente mas vazia" como caso especial
- Sem dado fake/seed na sidebar — estado vazio honesto até a primeira mensagem real (não existe
  persistência ainda, isso é a 6.6)
- `App.css` reescrito do zero: variáveis CSS customizadas em `:root` + `@media (prefers-color-scheme:
  dark)`, tema roxo vibrante (`#7c3aed`/`#6d28d9` no light, `#a78bfa`/`#8b5cf6` no dark) — atende o
  pedido do usuário (mesmo espírito do TruthID, mas roxo em vez de azul). Layout via CSS Grid
  (`280px 1fr`) pra sidebar + área de chat. Toda a demo antiga do scaffold (logos, `.row`,
  `.container`, cores hardcoded) removida
- Acessibilidade barata: `<form>` de verdade no input, `aria-label`s, lista de mensagens com
  `role="log" aria-live="polite"`, itens da sidebar como `<button>` nativo
- Removido o comando de exemplo `greet` (`#[tauri::command]` + registro em `invoke_handler!`) de
  `desktop/src-tauri/src/lib.rs` — boilerplate desconectado que a 6.3 não ia reaproveitar
- Verificação: `npm run build` (tsc + vite build) sem erro de tipo; `cargo build --workspace` e
  `cargo clippy --workspace --all-targets` limpos; `npm run tauri dev` rodado de verdade — dessa vez
  a tela não estava bloqueada, então consegui tirar screenshot da janela real: sidebar roxa, botão
  "+ New conversation", bolha de mensagem do usuário alinhada à direita em roxo vibrante. O usuário
  parece ter testado ele mesmo digitando "opa" enquanto o dev server rodava — confirma o fluxo
  completo (mensagem → bolha → conversa nomeada na sidebar) funcionando de ponta a ponta
- `PHASE.md` atualizado (6.2 concluída)

**Próximo passo**: 6.3 — integração com o core (IPC Rust↔frontend), plugando o `warden-core` de
verdade no `desktop/src-tauri` (que hoje só está no workspace estruturalmente, sem depender dele
ainda).

---

### 2026-08-02 — Sessão 16

- **Objetivo**: Etapa 6.3 — integração com o core (IPC Rust↔frontend). Mandar mensagem no chat
  agora chama o `Orchestrator` de verdade e mostra a resposta.

**O que foi feito**:

- Antes de plugar o desktop, identificado um problema real: `warden-cli/src/main.rs` já tinha
  ~90 linhas de lógica sensível (carregar config TOML, resolver provider/model/vault/API keys com
  precedência, montar `Orchestrator`+tools+sub-agente) que o desktop ia precisar duplicar
  inteirinha. Duplicar lógica de resolução de chave de API é exatamente o tipo de coisa que diverge
  silenciosamente com o tempo — não é abstração prematura extrair isso agora, já existem dois
  consumidores reais querendo o mesmo comportamento
- Planejado com um agente de arquitetura, que confirmou o desenho e leu o estado real de todos os
  arquivos envolvidos antes de finalizar
- Criado o crate `crates/warden-bootstrap` — recebe `FileConfig`/`ApiKeys`/`Provider` (com
  `Deserialize`, sem `ValueEnum` — não depende de `clap`), `load_config`/`resolve_secret`, e a
  função nova `bootstrap(explicit_config_path, overrides, default_vault_path) ->
  anyhow::Result<Orchestrator>`. Decisão deliberada: **não** foi pro `warden-core` (fonte de
  config é decisão de camada de canal, não do motor agnóstico de modelo) nem virou "warden-cli
  como lib" (misturaria parsing de CLI com algo que um app GUI também usa)
- `default_vault_path` é parâmetro da função, não hardcoded — o fallback certo difere por canal:
  CLI mantém `"vault"` relativo (comportamento idêntico ao de antes, usuário roda de onde quiser);
  desktop usa `~/Warden/vault` (absoluto — cwd de app lançado por ícone é imprevisível; vault fica
  direto na home por ser conteúdo navegável, tipo Obsidian, não escondido numa pasta de config)
- `warden-cli/src/main.rs` encolheu bastante: só ficou o `Cli` (clap), um `Provider` local com
  `ValueEnum` (convertido pra `warden_bootstrap::Provider` via `From`) e o loop REPL. Os 5 testes
  unitários de config migraram verbatim pro `warden-bootstrap`. `Cargo.toml` do `warden-cli`
  perdeu `dirs`/`serde`/`toml` (não usa mais direto) e ganhou `warden-bootstrap`
- **Problema de UX resolvido antes de acontecer**: no primeiro uso do desktop é bem provável que
  não exista `GEMINI_API_KEY` nem config file ainda (não existe tela de configuração — isso é a
  6.5). Se `bootstrap()` fosse chamado com `.expect()` antes do `tauri::Builder::run()`, o app
  nunca abriria janela nenhuma nesse caso — pior UX possível. Solução: `AppState { orchestrator:
  Result<Orchestrator, String> }` guardado no estado gerenciado do Tauri (sem `Mutex` — nada muta
  depois da construção, `Orchestrator` já é `Send + Sync`), e o comando `send_message` só propaga
  o erro quando a mensagem é de fato enviada. Janela sempre abre
- Frontend: `App.tsx` ganhou `isSending`/`sendError`, `handleSendMessage` virou `async` e chama
  `invoke<string>("send_message", { content })` — sucesso anexa a resposta como mensagem
  `assistant`, erro fica num banner (`sendError`) sem inventar uma bolha de assistente falsa.
  `ChatArea` mostra o banner (`role="alert"`) acima do input; `MessageInput` ganhou prop
  `disabled` (desabilita durante o envio, evita reenvio duplo)
- Verificação: `cargo build/test/clippy --workspace` limpos (22 testes — os 5 migrados pro
  `warden-bootstrap` + os 5 de processo de `warden-cli/tests/cli.rs` **sem nenhuma alteração**,
  confirmando que o refactor não mudou comportamento externo nenhum); `npm run build` (tsc+vite)
  limpo; testado de verdade via `npm run tauri dev` sem `GEMINI_API_KEY`/config file — janela abriu
  normal, sem crash, `~/Warden/vault` não foi criado (bootstrap falhou antes de tocar o
  filesystem, exatamente como esperado). Não deu pra testar o envio de mensagem de fato — sem
  `xdotool`/`wtype` funcionando nesse Wayland nativo (KDE Plasma), não há como simular digitação
  na janela a partir do terminal; fica pro usuário confirmar visualmente
- `PHASE.md` (6.3 concluída), `ARCHITECTURE.md` (decisão do `warden-bootstrap` registrada)

**Próximo passo**: 6.4 — canal nativo (chat direto no app, já praticamente pronto pós-6.3) ou 6.5
— configuração visual (tela de settings pra API keys/provider/vault, que hoje só dá pra editar via
`~/.config/warden/config.toml` na mão). Vale o usuário testar enviar uma mensagem de verdade
(com `GEMINI_API_KEY` setada) antes de seguir, já que isso não foi confirmado automaticamente
nesta sessão.

---

### 2026-08-02 — Sessão 3

- **Objetivo**: Etapas 1.2 e 1.3 — primeiro `ModelProvider` real (OpenAI) + loop de conversa via terminal.

**O que foi feito**:

- Registrada uma visão nova do usuário antes de implementar (`CONTEXT.md`, `ROADMAP.md`, `PENDING.md` P8):
  quer um canal Terminal completo estilo Claude Code (não focado em programação, foco em produtividade
  de comandos), cross-platform (Linux/Windows/Mac), com a tool `shell` (Fase 5.5) acessível a partir de
  **qualquer** canal, não só do terminal. Também confirmou interesse em "lançar agentes" — arquitetura
  ainda em aberto, ligado a sub-agentes leves (1.8) e à ideia de sub-agentes autônomos do `ROADMAP.md`.
- Implementado `OpenAiProvider` (`crates/warden-core/src/model/openai.rs`) — primeira implementação
  concreta de `ModelProvider`, chama `/v1/chat/completions` via `reqwest` (rustls, sem depender de
  OpenSSL do sistema — importa pro objetivo cross-platform)
- `warden-cli` agora roda um loop de conversa real: lê `OPENAI_API_KEY` do ambiente, lê linha de stdin,
  chama `Orchestrator::handle_message`, imprime a resposta; `exit`/`quit`/EOF encerram
- Testado: erro claro quando `OPENAI_API_KEY` não está setada; loop completo testado com key inválida
  (confirma que a request chega na API da OpenAI e erros são tratados sem derrubar o processo)
- `PHASE.md` atualizado (1.2 e 1.3 concluídas)

**Próximo passo**: 1.4/1.5 — plugar o `Vault` no orchestrator pra injetar contexto relevante no prompt
(decisão pendente P5: grep simples vs fuzzy finder). P3 (formato do system prompt) segue em aberto —
hoje só mensagens `user` são enviadas, sem persona configurável ainda.

---

### 2026-08-01 — Sessão 2

- **Objetivo**: Iniciar Fase 1 — setup do projeto Rust (etapa 1.1).

**O que foi feito**:

- Resolvida pendência P2: framework de CLI = `clap v4` (derive). `structopt` está descontinuado (incorporado ao clap desde a v3); `gum` é ferramenta Bash/TUI, não se aplica.
- Criado workspace Cargo com dois crates:
  - `crates/warden-core` — lib com os módulos `orchestrator`, `model`, `memory`, `tool`, cada um com a trait/struct base descrita em `ARCHITECTURE.md` (`ModelProvider`, `Tool`, `Vault`, `Orchestrator`)
  - `crates/warden-cli` — binário `warden`, usa `clap` para parsing de argumentos, ainda sem model provider real (placeholder)
- `cargo build` e `cargo run -- --vault-path ./vault` validados, workspace compila e roda limpo
- Atualizados `PHASE.md` (1.1 concluída), `PENDING.md` (P2 resolvida), `ARCHITECTURE.md` (linha de decisão do CLI framework)

**Próximo passo**: Etapa 1.2 — trait `ModelProvider` já existe, falta a implementação concreta `OpenAIProvider` (primeiro provedor) + carregamento de API key via config/env.

---

### 2026-07-31 — Sessão 1

- **Objetivo**: Estruturação inicial do projeto — criar pasta `project/` com documentação de planejamento, baseada no modelo do TruthID.

**O que foi feito**:

- Lido o documento de visão original (`warden-projeto.md`)
- Estudada a estrutura de planejamento do TruthID (`project/`)
- Criada pasta `project/` com 8 arquivos de documentação:
  - `INDEX.md` — índice do projeto
  - `OVERVIEW.md` — visão geral, stack, status das fases
  - `CONTEXT.md` — PRD (Product Requirements Document)
  - `GUIDELINES.md` — diretrizes de código e ensino
  - `ARCHITECTURE.md` — decisões de arquitetura
  - `PHASE.md` — 10 fases detalhadas de implementação
  - `PENDING.md` — pendências do projeto
  - `ROADMAP.md` — roadmap e evoluções planejadas
  - `SESSIONS.md` — este log de sessões
- Removido o arquivo original `warden-projeto.md`

**Próximo passo**: Iniciar Fase 1 (Fundação & Orquestrador) ou definir prioridades com o usuário.