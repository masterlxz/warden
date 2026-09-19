# Fases Detalhadas — Planejamento Global

> **Nota**: Este é o planejamento inicial do projeto, criado a partir do documento de visão original.
> As fases serão detalhadas e ajustadas conforme o progresso.

---

### Fase 1 — Fundação & Orquestrador

**Objetivo**: Ter um agente funcional via terminal/CLI — orquestrador falando com 1 modelo,
lendo/escrevendo num vault markdown local, sem canal externo ainda.

**Stack**: Rust (core), TypeScript (CLI se necessário)

**Etapas**:
- [x] 1.1 — Setup do projeto Rust + estrutura de módulos (orchestrator, model, memory, tool)
- [x] 1.2 — Trait `ModelProvider` + implementação OpenAI (primeiro provedor)
- [x] 1.3 — Loop de conversa via stdin/stdout (CLI)
- [x] 1.4 — Vault markdown local: ler/escrever arquivos `.md` em pasta configurável
- [x] 1.5 — Memória: buscar contexto relevante no vault (grep/ripgrep) e injetar no prompt
- [x] 1.6 — Trait `Tool` + primeira tool: `read_file`, `write_file`
- [x] 1.7 — Tool `web_search` (pesquisa na internet via API) — *implementação original (REST direto na API da Tavily) substituída pela versão via MCP na etapa 5.3*
- [x] 1.8 — Sub-agente leve: delegar tarefa escopada pra outro modelo/contexto — *estendido na Sessão 57 (P46, núcleo) pra suportar delegação recursiva com profundidade limitada; ver `PENDING.md` P46/P60*
- [x] 1.9 — Testes de integração do pipeline completo (CLI)
- [x] 1.10— Configuração via arquivo YAML/TOML (modelo, API keys, vault path)

**Decisões pendentes**: nenhuma — a última, formato do prompt de sistema (persona configurável),
foi resolvida na Sessão 43 (2026-09-03): agentes nomeados com persona em texto livre, registry no
`config.toml`, ver `PENDING.md` (fecha P3) e Fase 6 abaixo pra UI.

---

### Fase 2 — Canal Telegram

**Objetivo**: Primeiro canal externo funcionando — mais simples e sem risco de ban.

**Stack**: Rust (reqwest + Bot API) ou TypeScript

**Etapas**:
- [x] 2.1 — Setup do bot Telegram (token via `TELEGRAM_BOT_TOKEN`/config, long polling escolhido — ver `ARCHITECTURE.md`)
- [x] 2.2 — *Sem trait `Channel`* — decisão explícita do usuário; no lugar, função reutilizável `warden_bootstrap::handle_turn` (ver `ARCHITECTURE.md`). Trait de verdade fica pra quando o WhatsApp (Fase 3) existir
- [x] 2.3 — Receber mensagens e rotear para o orquestrador (`crates/warden-telegram/src/telegram.rs::run_bot`/`process_updates`/`handle_update`)
- [x] 2.4 — Enviar respostas de volta (`TelegramClient::send_message`, com split automático a cada 4096 caracteres)
- [x] 2.5 — *Texto puro, sem `parse_mode`* — decisão explícita do usuário. MarkdownV2 de verdade (conversor CommonMark→MarkdownV2) fica pra depois — ver `ARCHITECTURE.md`
- [x] 2.6 — Comandos básicos: /start, /help (resposta fixa, sem chamar o orchestrator)
- [x] 2.7 — Gerenciamento de conversas (thread por `chat_id`, via `handle_turn`/`load_conversation`, diretório próprio `conversations-telegram/`)
- [x] 2.8 — Testes de integração (hermáticos em `telegram.rs` via `ScriptedTelegramApi`, mais smoke tests de processo em `tests/telegram.rs`)

---

### Fase 3 — Canal WhatsApp

**Objetivo**: Segundo canal externo, usando Baileys como sidecar Node.

**Stack**: Node.js (Baileys), IPC com core Rust via socket local

**Etapas**:
- [x] 3.1 — Setup do sidecar Node.js com Baileys (`sidecar/whatsapp/`, primeiro código JS que o
  próprio projeto escreve e versiona — até aqui Node só era usado via `npx` contra pacotes de
  terceiros; `baileys@^7.0.0-rc14` — ver `ARCHITECTURE.md` pro porquê de não ser a linha `6.x` estável)
- [x] 3.2 — Autenticação via QR code — `useMultiFileAuthState`, QR gerado como **PNG** pelo pacote
  `qrcode` (não `qrcode-terminal`) dentro do diretório de auth, caminho logado no stderr do
  sidecar (canal separado do stdin/stdout usado pra IPC — ver
  `ARCHITECTURE.md`, um bug real de poluir o stdout foi encontrado e corrigido durante a
  verificação de ponta a ponta desta sessão)
- [x] 3.3 — IPC entre sidecar e core Rust — **stdin/stdout, JSON-lines** (decisão explícita do
  usuário entre as duas opções do `PHASE.md`; sem socket local). Ver `ARCHITECTURE.md`
- [x] 3.4 — Receber mensagens e rotear para o orquestrador (`crates/warden-whatsapp/src/
  sidecar.rs::run_bot`/`handle_event`, reusando `warden_bootstrap::handle_turn` da Fase 2)
- [x] 3.5 — Enviar respostas de volta (`WhatsAppSidecar::send`, comando `{"type":"send",...}`)
- [x] 3.6 — Gerenciamento de sessão (reconnect, keepalive) — reconexão via `DisconnectReason`
  fica inteiramente dentro do script Node (`connection.update`, reconecta a menos que
  `loggedOut`); o Rust só vê eventos de alto nível `connected`/`disconnected`
- [x] 3.7 — *Só degradação graciosa* — mensagem sem texto legível (imagem/áudio/documento) recebe
  uma resposta fixa, sem chamar o orchestrator. Suporte multimodal de verdade (`ModelProvider`/
  `Message` entendendo mídia) é mudança de model-layer, fora do escopo desta fase — ver `PENDING.md`
- [x] 3.8 — Testes de integração (hermáticos em `sidecar.rs` via `ScriptedSidecar`, mais smoke
  tests de processo em `tests/whatsapp.rs`; sem teste do lado do script Node — repo não tem
  toolchain de teste JS fora do frontend do desktop)

---

### Fase 4 — Vault & Memória

**Objetivo**: Memória persistente com backup descentralizado.

**Stack**: Rust, Arweave (via `pin()` do TruthID) — direção decidida na Sessão 50, motor
implementado na Sessão 54, ver P37 em `PENDING.md` e "Sync descentralizado (Fase 4)" em
`ARCHITECTURE.md`.

**Etapas**:
- [x] 4.1 — Motor de sync (`crates/warden-sync`) *(Sessão 54) — manifesto de diff incremental
  (hash por arquivo, `SyncManifest`/`SyncSecrets` em JSON separados do `config.toml`), envelope
  cifrado do bundle (AES-256-GCM via chave própria do Warden, derivada por HKDF, reaproveitando os
  primitivos genéricos de `warden-truthid::crypto`), cliente GraphQL do Arweave (descoberta da
  "última versão" por dono da carteira, já que `pin()` não permite tags customizadas), push/pull
  completos, e um protocolo de pareamento novo (código curto + LAN, sem câmera/QR — distinto do QR
  que o TruthID usa pra pagar) que espalha a chave de cifra entre os devices do usuário sem passar
  pelo TruthID/Arweave. 34 testes automatizados (unitários + round-trips de ponta a ponta contra
  telefone/gateway/par de pareamento falsos, mesmo padrão do `fake_phone.rs` do `warden-truthid`)*
- [x] 4.2 — Integração desktop (Tauri) *(Sessão 54) — tela "Sync" nova (status, botões Enviar/
  Pull, QR em SVG, fluxo de pareamento mostrar/digitar código), comandos IPC em
  `desktop/src-tauri/src/sync_cmds.rs`*
- [x] 4.3 — Integração CLI *(Sessão 54) — `/sync`, `/sync push` (QR em Unicode no próprio
  terminal, funciona por SSH numa máquina sem tela), `/sync pull`, `/sync pair` (mostrar código),
  `/sync pair <code>` (digitar código)*
- [x] 4.4 — Vault local + pareamento pleno no app mobile (Flutter) *(Sessão 55) — primeira ponte
  Rust↔Flutter do projeto: novo crate `crates/warden-mobile-bridge` (via `flutter_rust_bridge` +
  `cargokit`), casca fina sobre o mesmo `warden_sync::SyncEngine` do desktop/CLI, path explícitos
  resolvidos em Dart via `path_provider` (`getApplicationSupportDirectory()`). Nova
  `mobile/lib/screens/sync_screen.dart` (status, init, send/pull com QR via `qr_flutter`,
  pareamento host/join), reachable independente da conexão com `warden-server`. Override manual de
  host no join (não só sweep de LAN automático) — resolve o mesmo problema de NAT que a 7.2/7.3 já
  tinham com `10.0.2.2`, e é útil de verdade em wifi com isolamento de cliente, não só um hack de
  teste. Verificado de ponta a ponta num Android emulador real: `.so` compilado pras 4 ABIs via
  `cargo-ndk`, app instalado/aberto via `adb`, pareamento real contra um processo host separado
  (mesma chave de vault confirmada nos dois lados via `adb run-as`), e o motor de diff/hash rodando
  de verdade on-device (`pending_vault_changes` refletindo um arquivo escrito no vault local do
  emulador). Push/pull contra Arweave/TruthID reais e o lado iOS seguem sem teste — mesma lacuna já
  aceita em P38/P55 e P39/P44, ver `PENDING.md`
- [x] 4.5 — Busca semântica no vault *(Sessão 56) — embedding local via ONNX (`fastembed`,
  `AllMiniLML6V2`), decisão fechada com o usuário: local em vez de API, pra manter a busca offline
  e model-agnostic (Anthropic nem tem endpoint de embedding). `Vault::search_semantic` novo em
  `warden-core`, mesma `SearchHit` de sempre — `Orchestrator::handle_turn_streaming` tenta o
  caminho semântico (via `spawn_blocking`, primeiro uso desse padrão no projeto) e cai pro grep
  original em qualquer erro (ex. sem rede no primeiro download do modelo). Índice
  (`.warden/semantic_index.json`) vive dentro do próprio vault, dot-prefixado — já ignorado por
  `list_files`/`list_all_files`/sync sem precisar mexer em `warden-sync`. Verificado de ponta a
  ponta com o modelo real baixado de verdade nesta sessão (rede disponível no ambiente) — ranking
  correto distinguindo "consulta médica" de "compromisso com dentista" sem nenhuma palavra em
  comum*
- [x] 4.6 — Estrutura fixa/padrão do vault + visualização pela interface (P52, parte 1 + parte 2)
  *(Sessão 57) — três arquivos reservados na raiz do vault (`_profile.md`,
  `_behavior.md`, `_feedback.md`, cobrindo perfil do usuário/comportamento da IA/feedback e lições
  aprendidas, escolhidos explicitamente pelo usuário), seedados com um template curto em português
  na primeira vez que cada um é usado (`warden-bootstrap::seed_default_vault_files`, idempotente —
  nunca sobrescreve um arquivo já existente, então um vault restaurado via `warden-sync` de outro
  device não é tocado). `Vault::standing_memory()` novo em `warden-core` monta um bloco único a
  partir deles (pulando seção vazia/ausente) e `Orchestrator::handle_turn_streaming` injeta esse
  bloco como mensagem de sistema **sempre**, independente de busca por relevância — ao contrário do
  bloco de busca (grep/semântica) já existente, que só aparece quando há hit. Como o hook fica no
  único ponto real de implementação (`handle_turn_streaming`), todos os canais herdam de graça (CLI,
  desktop, Telegram, WhatsApp, mobile via `warden-server`), sem tocar em nenhum deles. Os 3 arquivos
  fixos passam a ser excluídos de `search`/`search_semantic` (evita duplicar o conteúdo já injetado
  fixo, e evita que consumam o orçamento de 8 hits da busca livre) mas continuam em
  `list_all_files`/sync normalmente — zero mudança em `warden-sync`. Verificado de ponta a ponta com
  o Gemini real (não mockado): editado `_profile.md` com um fato fictício ("tenho um dragão de
  estimação chamado Fumaça") sem nunca mencionar isso na conversa, perguntado sobre o "bicho de
  estimação" — resposta refletiu o fato corretamente, confirmando que a injeção chega no modelo de
  verdade. **Parte 2 — visualização pela interface** *(Sessão 57, continuação) — tela "Vault" nova
  no desktop (`VaultView.tsx`, entrada na sidebar ao lado de Usage/Sync/Settings), só leitura (edição
  continua por fora — Obsidian/editor de texto, ou a própria IA via `WriteFileTool`). Os 3 arquivos
  fixos aparecem destacados numa seção própria no topo da navegação (rótulos em português, mesma
  ordem de `standing_memory`), separados de uma árvore simples do resto do vault (pastas antes de
  arquivos, alfabética). Dois comandos Tauri novos (`desktop/src-tauri/src/vault_cmds.rs`, mesmo
  precedente de módulo dedicado que `sync_cmds.rs`): `list_vault_files`/`read_vault_file`, ambos
  reaproveitando o `Vault` já vivo em `AppState.orchestrator` (sem reconstruir um do zero por
  chamada). Conteúdo renderizado via `ReactMarkdown`/`remark-gfm`, dependência já existente no
  projeto (usada antes só em `MessageBubble.tsx`) — zero dependência nova, `MarkdownLink` exportado
  de lá e reaproveitado. Verificado via Playwright contra o dev server real (seção fixa, árvore por
  pasta, seleção trocando conteúdo renderizado, placeholder de vazio, claro/escuro) e também contra
  o vault real do usuário (`npm run tauri dev`, sem mock) — app abriu sem crash e os 3 arquivos
  fixos foram seedados de verdade em `~/Warden/vault/` (antes vazio)*
- [x] 4.7 — Sync automático ao reconectar (P71, Sessão 69, continuação) — **fatia 1: desktop,
  auto-pull**. Até aqui o motor de sync inteiro (Fase 4.1-4.6) era 100% manual — o usuário pediu
  que puxasse mudanças de outros dispositivos sozinho, sem precisar clicar. Confirmado com o
  usuário antes de codar: gatilho por checagem periódica (não detecção real de evento de
  reconexão do SO) e só desktop nesta fatia. `desktop/src-tauri/src/sync_cmds.rs::spawn_auto_pull`
  roda em `.setup()` do Tauri (primeiro hook desse tipo no app) — a cada 5 minutos (primeiro tick
  imediato, cobre "acabei de abrir o app"), pula silenciosamente se o sync nunca foi configurado
  neste device, senão chama `SyncEngine::pull()` de verdade e emite `auto-sync-pulled` pro
  frontend só quando algo realmente mudou. **Push continua manual em qualquer backend** — não é
  limitação desta fatia, é estrutural: o backend Arweave/TruthID sempre vai exigir aprovação física
  no celular (`finish_push` bloqueia nisso, trava de segurança proposital), e o único backend
  totalmente automatizável (git, P63) nunca foi ligado ao desktop, só existe no CLI. `SyncView.tsx`
  ganhou um listener pro evento novo, reaproveitando o mesmo padrão de `pairing-completed`/`app.
  emit` já usado no pareamento. Verificado com testes reais (não mockados): gate de "nunca
  configurado" provado sem nenhuma chamada de rede, e um `pull()` de verdade contra um gateway
  HTTP fake local (mesmo idioma de `crates/warden-sync/tests/fake_arweave_gateway.rs`) confirmando
  que o loop alcança a rede e completa. Ver `ARCHITECTURE.md` pro detalhamento completo e
  `PENDING.md` P71 pro que fica pra depois (push automático via git, mobile). **Fatia 2 (Sessão
  70, 2026-09-17) — mobile**: gatilho de foreground em vez de timer periódico, já que o processo
  Flutter não é um daemon de longa duração como o desktop. `ChatScreen` (já
  `WidgetsBindingObserver` desde a notificação local, Fase 7.5) ganhou `_autoPullOnResume()`,
  disparado só na transição *para* `resumed` (nunca ao permanecer nele, nunca no primeiro frame) —
  gate isolado numa função pura nova, `mobile/lib/services/sync_auto_pull.dart::
  shouldAutoPullOnResume`, mesmo padrão de `chat_notifications.dart::shouldNotifyFor`. Checa
  `bridgeStatus(...).paired` antes de chamar `bridgePull(...)` (`bridgeStatus` é seguro mesmo sem
  sync nunca configurado — arquivo ausente vira `Ok(None)`/default em `warden-sync::manifest`, não
  erro) — pula silenciosamente se nunca pareado, mesmo espírito do gate `is_initialized()` do
  desktop. Reação de UI é um `SnackBar` (mais leve que o banner passivo do `SyncView.tsx`) só
  quando algo mudou de fato, mensagem montada por outra função pura,
  `autoPullMessageFor`. Qualquer outro erro (ex. sem rede) só vai pro `debugPrint`, nunca
  interrompe o chat — mesma postura do `eprintln!` do desktop. `SyncScreen` (pull manual) não
  mudou. Zero mudança em Rust/FFI — `bridge_status`/`bridge_pull` já existiam e já se comportavam
  do jeito necessário. Verificado com `flutter analyze`/`flutter test` (52 testes, os 8 novos de
  `sync_auto_pull_test.dart` inclusos) limpos — sem emulador Android real disponível neste
  ambiente pra confirmar o SnackBar aparecendo de fato num device, mesma lacuna já aceita em
  outras fatias mobile (ver `PENDING.md` P70). Fecha a parte "mobile" do P71 — fica só o push
  automático via git (desktop) em aberto. **Fatia 3 (Sessão 72, 2026-09-18) — push automático via
  git no desktop, fecha o P71 por completo**: o `GitSyncEngine` (P63) já existia e funcionava de
  ponta a ponta no CLI (`/sync git push`/`pull`), mas nunca tinha ganhado nenhuma camada desktop.
  Settings: `SettingsSnapshot`/`SettingsFormPayload` ganharam `git_sync` (mesmo padrão exato de
  `remote_node` — payload dedicado camelCase, validação tudo-ou-nada em `save_settings`), seção
  nova "Sync via Git" no `SettingsView.tsx` (`GitSyncForm`, 2 campos: Remote URL + Token via
  `ApiKeyField`). Comandos Tauri novos (`desktop/src-tauri/src/git_sync_cmds.rs`,
  `git_sync_configured`/`git_sync_push`/`git_sync_pull`) constroem um `GitSyncEngine` fresco por
  chamada — mesma composição de paths que `make_git_sync_engine` do CLI já usava. `SyncView.tsx`
  ganhou uma seção "Git" com botões Push/Pull (sem fluxo de QR — diferente do Arweave, aqui não há
  aprovação humana no meio) e um hint apontando pra Settings quando `git_sync` não está
  configurado. **Auto-sync**: `sync_cmds::spawn_auto_pull` renomeado pra `spawn_auto_sync` e
  estendido — a cada tick relê `config.toml` fresco (achado que definiu o design: Arweave e git
  compartilham o mesmo `secrets_path`/`manifest_path` em disco, então são backends alternativos,
  não aditivos — só um roda por tick, decidido pela presença de `config.git_sync`); configurado,
  faz `pull()` então `push()` via git (pull primeiro pra nunca bater num push rejeitado por
  non-fast-forward à toa), emitindo `auto-sync-pulled`/um evento novo `auto-sync-pushed` só quando
  algo mudou de verdade; sem `git_sync`, comportamento idêntico ao de antes (só Arweave). Push do
  Arweave continua inteiramente manual — `finish_push` segue bloqueando numa aprovação física no
  celular, trava de segurança que esta fatia não toca. `build_git_sync_engine` (helper
  compartilhado entre os comandos e o loop) recebe `secrets_path`/`manifest_path`/`git_repo_path`
  como parâmetros explícitos em vez de resolvê-los internamente via `warden_sync::paths`. **Bug
  real pego pelo teste de integração, não só um risco teórico**: a primeira versão só
  externalizava `secrets_path`/`manifest_path`, deixando `git_repo_path` resolvido internamente —
  o teste de pull-então-push passou na primeira rodada mas falhou na segunda
  (`git checkout --orphan main falhou: a branch named 'main' already exists`), porque o clone local
  de trabalho do `GitSyncEngine` é compartilhado entre todo push/pull do device por design
  (`git.rs`'s doc comment), então cada rodada de teste reaproveitava o `~/.config/warden/
  git-sync-repo` real desta máquina, com `main` já commitado pela rodada anterior — o mesmo
  problema que teria poluído o `sync_secrets.json`/`sync_manifest.json` reais se aqueles dois não
  tivessem sido externalizados desde o início. Corrigido externalizando também `git_repo_path`;
  suíte rodada 3x seguidas depois pra confirmar que a flakiness sumiu, e `~/.config/warden/`
  conferido sem `git-sync-repo`/`sync_secrets.json` novos depois da rodada limpa. Verificado com
  `cargo check/clippy/test -p desktop` (15 testes, 4 novos: serialização camelCase dos payloads
  novos, e dois testes não mockados do branch git do auto-sync contra um bare repo git local de
  verdade — um confirma o gate "nunca inicializado" nunca toca a rede, outro confirma um
  pull-então-push real produzindo um commit) e `tsc`/`npm run build` do desktop limpos. Sem host
  git remoto real (Gitea/GitHub) nem uma janela Tauri real disponíveis neste ambiente pra clicar os
  botões novos de ponta a ponta — mesma lacuna já aceita nas fatias anteriores de P71/P63. **Fecha
  o P71 por completo**

---

### Fase 5 — Tools & MCP

**Objetivo**: Sistema de ferramentas extensível, compatível com MCP.

**Stack**: Rust (core), qualquer linguagem para MCP servers

**Etapas**:
- [x] 5.1 — Registry de tools (`ToolProvider` trait)
- [x] 5.2 — MCP client: conectar em MCP servers externos
- [x] 5.3 — Tool `web_search` via MCP
- [ ] 5.4 — Tool `browser` (via extensão — ver Fase 8)
- [x] 5.5 — Tool `shell` (executar comando no nó cliente)
- [x] 5.6 — Tool `file_system` (ler/escrever arquivos no nó cliente) — *sem código novo: realizada via o mecanismo genérico `[[mcp_servers]]` da 5.2, apontando pro server oficial `@modelcontextprotocol/server-filesystem`; ver `ARCHITECTURE.md`*
- [x] 5.7 — Integração Google (Gmail, Drive, Calendar) via MCP servers existentes — *sem código novo, mesmo mecanismo genérico `[[mcp_servers]]` da 5.2/5.6; server escolhido `@aaronsb/google-workspace-mcp` (`npx`); ver `ARCHITECTURE.md`*
- [x] 5.8 — Tracking de uso/token por mensagem *(escopo reduzido, decisão explícita do usuário:
  só captura e persistência de tokens (`Usage`), sem rate limiting nem teto de gasto configurável
  — isso continua em aberto, ver `PENDING.md` P4)*

- [x] 5.9 — Skills (P16, Sessão 73): pacotes de instrução em `skills/<nome>.md` no vault, catálogo
  no prompt + `use_skill` sob demanda, criação pela conversa (`manage_skill`), à mão e por prompt na
  tela Skills do desktop — ver `PENDING.md` P16 (resolvida), P72 (fora do escopo), P73 (sem teste com
  modelo real) e `ARCHITECTURE.md`

**P11 resolvida (Sessão 35, 2026-08-29)** — as duas direções da tela de gerenciamento de MCP:
(a) UI de verdade no desktop pra adicionar/editar/apagar servers MCP, com 4 presets de
"quick add" (Filesystem, Google Workspace, Notion, GitHub via Docker); (b) novo binário
`warden-mcp-server`, expõe as tools do orchestrator como um server MCP de verdade sobre stdio
pra qualquer client de terceiro (Claude Desktop, etc.) se conectar. Ver `ARCHITECTURE.md` e
`PENDING.md` (P25 nova: client MCP só fala stdio, bloqueou um preset de Slack).

**P25 resolvida (Sessão 36, 2026-08-31)** — client MCP ganhou transporte HTTP
(`McpToolProvider::connect_http`), ao lado do `connect_stdio` já existente; `[[mcp_servers]]`
aceita tanto entradas `command`/`args`/`env` (stdio) quanto `url`/`headers` (HTTP), e a UI de
Settings do desktop ganhou um seletor de transporte por server. Verificado de ponta a ponta
contra um server HTTP real (não mockado). Resolvida só parcialmente — falta um client OAuth
pra cobrir servers (Slack incluso) que não aceitam um token estático, ver `PENDING.md` P26.

**P26 resolvida (Sessão 37, 2026-08-31)** — client MCP ganhou um client OAuth de verdade
(discovery, Dynamic Client Registration, PKCE, renovação automática), quase inteiramente
provido pela feature `auth` do próprio `rmcp` (3.1.4). Novo módulo `tool/mcp_oauth.rs`:
`connect_http_oauth` (headless, todo `bootstrap()`) e `authorize_interactively` (interativo,
botão "Connect" da Settings). `McpServerConfig::Http` ganhou `oauth: bool`. Verificado de ponta
a ponta contra um server OAuth real local (`tests/mcp_oauth.rs`) — discovery, DCR, troca PKCE,
persistência em disco e reconexão headless, tudo passou de primeira. Ver `ARCHITECTURE.md` e
`PENDING.md` pros detalhes e pro que ainda falta (teste contra o Slack real).

---

### Fase 6 — App Desktop Nativo

**Objetivo**: Aplicação desktop nativa com Tauri — mesma stack do TruthID.

**Stack**: Tauri + Rust + React + TypeScript

**Etapas**:
- [x] 6.1 — Setup Tauri + React + TypeScript
- [x] 6.2 — Shell do app: sidebar de conversas, área de chat
- [x] 6.3 — Integração com o core (IPC Rust↔frontend)
- [x] 6.4 — Canal nativo (chat direto no app)
- [x] 6.5 — Configuração visual (modelo, API keys, canais)
- [x] 6.6 — Histórico de conversas
- [x] 6.7 — Renderização de markdown nas mensagens
- [x] 6.8 — Build Linux/Windows/macOS

**Identidade visual** (usuário, 2026-08-02): estética parecida com a do TruthID, mas em roxo
vibrante em vez de azul. Vale a partir da 6.2, quando a UI de verdade começa a ser construída — o
scaffold da 6.1 ainda é só o boilerplate padrão do template Tauri+React+Vite, sem branding.

**Polish pós-conclusão (Sessão 35, 2026-08-29)**: re-priorização do usuário — voltar ao Desktop
antes de seguir pro resto do roadmap (ver `ROADMAP.md`). Settings ganhou um registry de
provedores de verdade (Gemini/OpenAI/Anthropic/qualquer servidor OpenAI-compatível como Ollama —
ver P22/P23 em `PENDING.md`, resolvidas, e as decisões correspondentes em `ARCHITECTURE.md`).

**Restyle do chat (Sessão 39, 2026-08-31)**: reformulação visual completa puxando pro estilo
"AI product" (ChatGPT-like) — ver `ARCHITECTURE.md`.

**Anexo de imagem no chat (Sessão 40, 2026-09-01)**: metade "imagem" de P28 resolvida — composer
ganhou botão de anexo, `Message`/`ModelProvider` (`warden-core`) ganharam suporte multimodal real
pros três providers. Ver `ARCHITECTURE.md` e `PENDING.md`.

**Input de voz no chat (Sessão 41, 2026-09-01)**: metade "input" de áudio de P28 resolvida —
composer ganhou botão de microfone, transcrição via Whisper (chave dedicada, independente do
provider de chat ativo). Ver `ARCHITECTURE.md` e `PENDING.md` (P28 fica só com TTS na resposta em
aberto; P29/P30 são os testes de ponta a ponta ainda pendentes pros dois anexos).

**TTS na resposta (Sessão 42, 2026-09-03)**: última metade de P28 resolvida — cada resposta do
assistente ganhou um botão de áudio (manual, não autoplay) que sintetiza fala via a mesma API de
voz da OpenAI (`/v1/audio/speech`, chave `whisper` reaproveitada) e toca no próprio composer. P28
fecha de vez (as três metades — imagem, input de voz, TTS — feitas). Ver `ARCHITECTURE.md` e
`PENDING.md` (P29/P30/P31 seguem como os três testes de ponta a ponta ainda pendentes, todos
bloqueados por falta de chave de API real no shell do agente). Sem item novo de UX geral pendente
além do que já está registrado em `PENDING.md`.

**Agentes nomeados + seletor por conversa (Sessão 43, 2026-09-03)**: fecha P3 (persona
configurável, em aberto desde a Fase 1) — nova seção "Agents" na Settings (nome + personalidade
em texto livre + modelo padrão opcional) e uma barra `.chat-header` nova no topo do chat com dois
seletores (Agent/Model), escolhíveis por conversa e persistidos nela. Ver `ARCHITECTURE.md` e
`PENDING.md` (P32 é o teste de ponta a ponta ainda pendente, mesma lacuna de chave de API real que
P29/P30/P31).

**Um agente por conversa, travado na criação (Sessão 57, 2026-09-08)**: fecha P45 — substitui o
seletor de agente do `chat-header` (Sessão 43 acima), que deixava trocar de agente a qualquer
momento, por uma escolha única antes da primeira mensagem. Conversa nova sem agente escolhido
ainda mostra um `.agent-picker` (cards clicáveis, um por agente cadastrado, ou um direcionamento
pra Settings se nenhum agente existir) no lugar do composer liberado; escolhido o agente, vira uma
conversa normal com o nome do agente fixo (rótulo somente-leitura, não mais `<select>`) no header
pro resto da vida da conversa. Escopo confirmado com o usuário: só desktop (Telegram/WhatsApp/
mobile/CLI não têm — ou mantêm — seletor de agente, fora de escopo); modelo/provider continua
livre pra trocar a qualquer momento, só o agente trava; agente passou a ser obrigatório, não existe
mais opção "sem agente" numa conversa nova. Nenhuma mudança de schema/Rust — `Conversation.agent_id`
já existia por-conversa desde a Sessão 43, a trava é inteiramente de UX no `ChatArea.tsx`/`App.tsx`.
Conversas antigas (criadas antes desta feature) continuam funcionando normalmente, só sem a trava
nova sobre um histórico que não a respeitava. Ver `ARCHITECTURE.md`.

---

### Fase 7 — App Mobile

**Objetivo**: Warden no celular como cliente (nunca servidor).

**Stack**: ~~Tauri Mobile (Rust + React)~~ **Flutter (Dart)** — trocado na Sessão 50
(continuação): usuário priorizou maturidade geral e suporte a iOS acima do reuso de UI que
motivou a escolha original do Tauri. Client fala o protocolo WS/JSON do servidor (decisão P1,
`ARCHITECTURE.md`) — decisão agnóstica de framework, não afetada por essa troca. Ver
`ARCHITECTURE.md` ("Mobile: troca de Tauri Mobile pra Flutter") e `PENDING.md` P35.

**Etapas**:
- [x] 7.1 — Setup Flutter (Android + iOS) — *substitui o setup anterior em Tauri Mobile
  (revertido, Sessão 50 continuação): scaffold `gen/android/` removido, projeto novo em
  `mobile/` (fora de `desktop/`). Lado **Android** verificado de ponta a ponta (build real +
  emulador + screenshot); toolchain Android de `~/.local/opt/` reaproveitado, não reinstalado.
  Lado **iOS** só com o projeto Xcode gerado (`mobile/ios/`), nunca buildado/testado — sem
  Xcode/macOS neste container, ver `PENDING.md` P39. O achado de que a UI fixa do desktop não
  serve num celular continua válido — reforça que a 7.3 sempre ia precisar de um layout mobile
  dedicado*
- [x] 7.2 — Conectar ao servidor (Tailscale + WebSocket) — *protocolo já fechado
  na 9.2 (WS + JSON próprio, decisão P1); client Dart em `mobile/lib/` mirando
  `crates/warden-server/src/protocol.rs`. Escopo estreito de propósito: prova
  conectividade (handshake, heartbeat, erro de auth), não é a UI de chat
  (isso é a 7.3). Verificado de ponta a ponta contra um `warden-server` real
  (não mockado) rodando no host, do emulador Android via `10.0.2.2` — os três
  caminhos (handshake OK, `Goodbye` limpo, chave errada rejeitada) conferidos
  com screenshot real via `adb`. Sem Tailscale real disponível neste ambiente
  (mesma lacuna já aceita do lado servidor, `PENDING.md` P36, agora estendida
  ao cliente mobile)
- [x] 7.3 — Interface de chat mobile *(Sessão 51) — chat de verdade, não mock: `warden-server`
  passou a hospedar um `Orchestrator` real (reabrindo a decisão da 9.2 de não ter um) e responde
  `Chat` com o modelo de verdade, uma conversa por `device_id` (mesmo padrão do Telegram/
  WhatsApp). Novo `ChatScreen` no Flutter, verificado de ponta a ponta contra um `warden-server`
  real com Gemini de verdade. Sem fetch de histórico ao reconectar ainda (`PENDING.md` P40) — só a
  UI de chat em si, layout mobile mais completo (seletor de agente/provider, etc.) fica pra depois*
- [x] 7.4 — Execução de tools local *(Sessão 52) — acesso a arquivos do celular, só leitura
  (`list_phone_files`/`read_phone_file`), pasta raiz persistida via SAF (Android). "Shell" saiu do
  escopo (não existe num celular sem root). Mecanismo real: `warden-server` ganhou um roteamento
  de tool genérico pro cliente conectado certo (`RemoteTool`, `crates/warden-server/src/
  remote_tool.rs`) — primeira peça concreta do que a Fase 9.4/9.5 vai generalizar depois.
  Verificado de ponta a ponta contra um `warden-server` real com Gemini de verdade: listou e leu
  arquivos reais empurrados pro emulador via `adb push`*
- [x] 7.5 — Notificações push (locais) *(Sessão 56) — decisão do usuário: notificação local via
  `flutter_local_notifications` (app vivo em background), não push de verdade via FCM/APNs — evita
  a primeira dependência de nuvem de terceiro do projeto inteiro. Gatilho em `ChatScreen`
  (`WidgetsBindingObserver` + `AppLifecycleState`, já que não existe hoje nenhum outro jeito de
  navegar pra fora do chat enquanto conectado — P41). `mobile/lib/services/chat_notifications.dart`
  novo, funções puras (`shouldNotifyFor`/`notificationContentFor`) testadas sem platform channel,
  mesmo padrão pure-vs-plugin do `warden-cli`. Achado no meio do caminho, sem relação com a decisão
  em si: `fastembed` (Fase 4.5) quebrava a compilação cruzada do `warden-mobile-bridge` pra Android
  — `ort` sem binário pré-compilado pra `armv7-linux-androideabi`, e `native-tls`/`openssl-sys` sem
  build pro alvo. Corrigido tornando `semantic-search` uma feature opcional em `warden-core`
  (default-on), desligada só em `warden-sync` (que nunca usa `Orchestrator`/chat, só I/O de
  arquivo), e trocando o TLS do `fastembed` pra rustls. **Verificado de ponta a ponta contra
  hardware real (emulador Android), zero mock**: prompt de permissão de notificação real aceito,
  mensagem mandada por um `warden-server` real (chave OpenAI inválida de propósito, pra ter uma
  resposta rápida e determinística), app levado pro background antes da resposta chegar, e a
  notificação real do Android apareceu na bandeja com o conteúdo certo — confirmado por
  `dumpsys notification`, screenshot da bandeja puxada, e toque na notificação reabrindo o chat com
  a conversa intacta*
- [ ] 7.6 — Build e deploy (release assinado, publicação nas lojas) — fora do escopo desta sessão,
  não pedido pelo usuário; só o fluxo `--debug` de sempre (7.1-7.5) foi usado

---

### Fase 8 — Extensão de Navegador

**Objetivo**: Extensão Chrome/Firefox que funciona como canal de chat + tool provider de browser.

**Stack**: Web Extension (Manifest V3), TypeScript

**Etapas**:
- [x] 8.1 — Setup da extensão (Manifest V3, popup, background script) *(Sessão 68, continuação) —
  `extension/` novo (raiz do repo, irmão de `desktop`/`mobile`), Vite + React 19 + TS (mesma stack
  de `desktop/`) mais `@crxjs/vite-plugin` (empacota Manifest V3 a partir do Vite — puro Vite não
  gera manifest/service worker compatíveis). Chrome-only nesta fatia (Firefox fica pra 8.7/8.8,
  mesma postura "uma plataforma primeiro" que a 7.1 do mobile teve com Android antes de iOS)*
- [x] 8.2 — Canal de chat (popup com conversa) *(Sessão 68, continuação) — cliente WS em
  TypeScript (`extension/src/background/connection.ts`), porta 1:1 de
  `mobile/lib/services/server_connection.dart` (Fase 7.2/7.3): handshake com timeout, heartbeat
  `Ping`/`Pong`, `sendChat`. A conexão mora no **background service worker**, não no popup (que a
  MV3 destrói ao fechar) — heartbeat a 20s (mais apertado que os 30s do mobile) aproveita que
  Chrome 116+ reseta o timer de ociosidade do service worker a cada troca de mensagem no
  WebSocket, evitando que o SW seja descartado enquanto conectado. Isso já cobre a substância da
  8.7 (comunicação via WebSocket) pro caminho de chat — falta só estender pra tool calls quando
  8.3-8.6 existirem. Sem reconexão automática nem histórico persistido entre reinícios do SW —
  mesmo corte de escopo que a 7.2 do mobile aceitou, ver `PENDING.md`. **Verificado de ponta a
  ponta contra um Chrome/Brave e um `warden-server` reais na Sessão 68, continuação 2** — ver
  `ARCHITECTURE.md`/`PENDING.md` P67*
- [x] 8.3 — Tool provider: ler DOM da página ativa *(Sessão 68, continuação 3) —
  `browser_read_page`, ver detalhamento nas notas de 8.3-8.7 abaixo*
- [x] 8.4 — Tool provider: clicar em elementos *(Sessão 68, continuação 3) — `browser_click_element`*
- [x] 8.5 — Tool provider: navegar para URL *(Sessão 68, continuação 3) — `browser_navigate`*
- [x] 8.6 — Tool provider: extrair texto/seleção *(Sessão 68, continuação 3) — `browser_extract_text`*
- [x] 8.7 — Comunicação com o servidor Warden (WebSocket) *(Sessão 68, continuação 3) —
  transporte já existia desde a 8.2; fechado o roteamento de `ToolCallRequest`/`ToolCallResult`/
  `ToolCallError` que as 8.3-8.6 precisavam. Nenhum código novo no `warden-server`/`warden-core`
  pro roteamento em si — o mecanismo genérico de tool local por conexão (Fase 7.4, já usado pelo
  mobile) cobriu de graça, ver `ARCHITECTURE.md`. Verificado de ponta a ponta contra um
  Chrome/Brave e um `warden-server` reais — achado no caminho: bug real e pré-existente no
  provider Gemini (`role: "function"` rejeitado pela API atual, corrigido pra `role: "user"`),
  não relacionado à extensão em si, ver `ARCHITECTURE.md`/`PENDING.md`*
- [ ] 8.8 — Publicação na Chrome Web Store / Firefox Add-ons

---

### Fase 9 — Rede de Nós & Tailscale

**Objetivo**: Múltiplos clientes conectados ao servidor, execução remota de tools.

**Stack**: Tailscale, WebSocket/gRPC, Rust

**Etapas**:
- [x] 9.1 — Descoberta de hub na rede local (redefinida pelo usuário, Sessão 69 continuação — não é
  "setup Tailscale": é o Warden encontrar sozinho outros dispositivos na LAN, sem digitar host/porta
  à mão; Tailscale segue como infra opcional do próprio usuário, fora do código do Warden, ver
  `ARCHITECTURE.md`) — **fatia 1 (esta sessão)**: protocolo (`ClientMessage::Discover`/
  `ServerMessage::DiscoverAck`), servidor (`handle_connection` responde sem exigir `auth_key`, nunca
  registra a sonda como dispositivo), sweep reaproveitando `warden_truthid::lan::candidate_hosts()`
  (`warden-server-protocol::discovery`), e botão "Procurar hubs na rede" no `WorkspaceView.tsx` do
  desktop. Verificado de ponta a ponta com um `warden-server` real bindado em `0.0.0.0:7420` —
  achado pela IP real da LAN da máquina, não só loopback. **Fatia 2 (mesma sessão, continuação)**:
  mobile — `warden-mobile-bridge` ganhou `bridge_discover_hubs(port)` (mesma
  `warden_server_protocol::discover_hubs` do desktop, via FFI), `ConnectionScreen` ganhou um botão
  de descoberta (bottom sheet com a lista de hubs achados) ao lado do scanner de QR. Verificado com
  `cargo build/test/clippy --workspace` e `flutter analyze`/`flutter test` (44 testes) limpos — sem
  build/teste num emulador Android real nesta fatia (precisaria reinstalar `cargo-ndk`, que não
  persistiu neste ambiente; mecanismo de sweep em si já provado de ponta a ponta na fatia 1).
  **Fatia 3 (mesma sessão, continuação) — extensão de navegador**: como ela não chama Rust,
  `extension/src/background/discovery.ts` reimplementa o mesmo sweep em TypeScript falando o
  protocolo `Discover`/`DiscoverAck` idêntico contra o servidor inalterado (`chrome.system.network.
  getNetworkInterfaces()`, permissão `system.network` nova no manifest, `WebSocket` curto por
  candidato com concorrência limitada). Botão "Procurar hubs na rede" no `ConnectionForm.tsx`.
  `npx tsc --noEmit`/`npm run build` limpos; sem verificação manual num Chrome/Brave real (mesma
  lacuna já aceita em P67/P68). **Fecha as três fatias da 9.1** — ficam só as limitações menores
  em `PENDING.md` P70: porta fixa (7420 em todas as três), build Android real (fatia 2)
  **Atualizado (Sessão 71, 2026-09-18)**: porta fixa resolvida — o motor de sondagem em si já era
  parametrizado por porta nas três linguagens (`discover_hubs(port: u16)` em Rust,
  `bridgeDiscoverHubs({required int port})` no mobile bridge, `discoverHubs(port: number)` no
  TypeScript da extensão); o problema estava só na camada de UI/wrapper de cada cliente, que
  ignorava esse parâmetro e sempre sondava `7420`. Desktop: comando Tauri `discover_hubs` passou a
  receber `port` do frontend (`WorkspaceView.tsx` ganhou um campo "Porta a procurar", default
  `"7420"`, antes só existia `serverUrl` como texto livre). Mobile: `_discoverHubs()`
  (`connection_screen.dart`) passou a ler o mesmo `_portController` já usado pro connect manual, em
  vez de sempre `ConnectionSettingsStore.defaultPort` — nenhuma UI nova. Extensão: `PopupRequest`'s
  `discoverHubs` ganhou `port`, `ConnectionForm.tsx` manda o valor já digitado no campo "Porta"
  existente, `background/index.ts` usa `request.port` (constante `DISCOVERY_PORT` removida, ficou
  morta). `cargo check/clippy/test -p desktop`, `tsc`/`npm run build` (desktop e extensão),
  `flutter analyze` todos limpos. Sem Chrome/Brave real, hub numa porta não-default nem emulador
  disponíveis neste ambiente pra confirmar visualmente em runtime — mesma lacuna já aceita em
  P67/P68/P70. Seguem em aberto só (2) build Android real e (3) verificação manual da extensão
- [x] 9.2 — Protocolo servidor↔cliente — WebSocket + JSON próprio (handshake + heartbeat só;
  roteamento de tool pra 9.4/9.5), ver `ARCHITECTURE.md` e `PENDING.md`
- [x] 9.3 — Registrar cliente no servidor (pareamento) *(Sessão 60, continuação)* — registro
  **persistente** em JSON (`crates/warden-server/src/device_registry.rs`, `PairingStore`),
  substituindo o registro efêmero em memória (Sessão 59). Escopo confirmado com o usuário: a
  aprovação vale só pro **roteamento** (`CallDeviceTool`) — `Hello`/`Chat`/`Ping` seguem
  funcionando pra qualquer dispositivo com o `auth_key` certo, sem exigir aprovação prévia.
  Gerenciamento via CLI (`warden-server devices list/approve/revoke`, sem UI ainda — isso é 9.6)
- [x] 9.4 — Rotear requisição de tool para o cliente correto — registro de dispositivos conectados
  + `CallDeviceTool`/`DeviceToolResult`/`DeviceToolError` no `warden-server` (Sessão 59), testado
  com dois clientes reais na mesma suíte de integração; falta o lado cliente que de fato usaria
  isso (ex. P61's `RemoteNodeProvider`) — ver `PENDING.md`
- [x] 9.5 — Cliente executa tool localmente e devolve resultado — `warden-node` (Sessão 59,
  `crates/warden-server/src/bin/warden-node.rs`), o agente-de-nó real do P61: conecta como cliente,
  anuncia `vault_read`/`vault_write`/`vault_list`/`vault_delete`, executa contra seu próprio `Vault`
  local e devolve o resultado — escopado só a essas 4 tools de vault, não um nó genérico de
  qualquer tool ainda; ver `PENDING.md` P61
- [x] 9.6 — Workspace de máquinas (ver/gerenciar nós conectados) *(Sessão 60, continuação)* — tela
  nova no desktop (`WorkspaceView.tsx`), lista/aprova/revoga dispositivos do `PairingStore` (9.3).
  Escopo confirmado com o usuário: assume que o desktop roda na **mesma máquina** do
  `warden-server` (lê `devices.json` local direto via nova dependência no crate `warden-server`),
  não uma superfície admin nova no protocolo WS — isso fica pra quando um cenário multi-máquina
  de verdade existir
- [x] 9.7 — Pareamento de cliente novo via QR code *(Sessão 61, continuação)* — direção escolhida
  com o usuário: **desktop mostra o QR, mobile escaneia** (não o inverso do TruthID — quem precisa
  aprender host/porta/chave é o cliente novo). Novo arquivo local `hub_pairing.json`
  (`warden_bootstrap::HubPairingConfig`/`default_hub_pairing_config_path`, fora do `config.toml`
  principal, mesmo espírito de arquivo dedicado que `devices.json` já tinha) guarda o server URL +
  auth key que o operador digita uma vez na nova seção "Pareamento por QR" do `WorkspaceView.tsx`;
  três comandos Tauri novos em `workspace_cmds.rs` (`get_hub_pairing_config`/
  `save_hub_pairing_config`/`hub_pairing_qr_svg`) geram o SVG reaproveitando `render_qr_svg`
  (extraído de `sync_cmds.rs` pro módulo novo `qr.rs`, evitando duplicar a chamada ao crate
  `qrcode`). Payload do QR é só `{serverUrl, authKey}` — sem `deviceId`, que continua escolhido
  pelo próprio cliente. Lado mobile: `mobile_scanner` novo (`pubspec.yaml`, mais a permissão
  `CAMERA` no `AndroidManifest.xml`), `parseHubPairingQr` (`hub_pairing_qr.dart`) isolado como
  função pura testável sem câmera (mesmo padrão de `chat_notifications.dart::shouldNotifyFor` da
  Fase 7.5), `QrScanScreen` novo, botão de câmera na `ConnectionScreen` que preenche host/porta/
  chave sem auto-conectar. Zero mudança em `warden-server-protocol`/`PairingStore` — o QR só evita
  digitação, a aprovação do dispositivo continua manual no Workspace (9.3/9.6), inalterada.
  Verificado: `cargo build/test/clippy --workspace` limpos (round-trip de
  `load_hub_pairing_config`/`save_hub_pairing_config`, formato JSON do payload travado por teste);
  `npm run build` (tsc+vite) limpo no desktop. SDK Flutter instalado depois, na continuação
  seguinte desta mesma sessão (`~/.local/opt/flutter`, stable, via `PATH` em `~/.bashrc`):
  `flutter pub get` resolveu `mobile_scanner 7.4.1` de verdade, `flutter analyze` limpo, `flutter
  test` verde (39 testes, os 6 novos de `parseHubPairingQr` inclusos). JDK 21 + Android SDK
  (cmdline-tools/platforms 35+36/build-tools) instalados na sequência com o ok do usuário; achada a
  causa raiz do disco cheio no meio do caminho — `warden/target` do Cargo sozinha tinha **76GB** no
  disco principal, junto de `~/.gradle`/`~/.pub-cache`/`mobile/build`. Resolvido de vez movendo tudo
  isso (mais `~/.cargo`/`~/.rustup`, instalado nesta sessão pro `cargokit` cross-compilar) pro HD de
  1TB (`/mnt/hd1tb/dev-tools/`) com symlinks no lugar de sempre — disco principal caiu de 99% pra
  ~53% de uso. Com espaço de sobra e o `rustup` (+ targets Android) instalado, `flutter build apk
  --debug` **compilou de verdade**: `warden_mobile_bridge` (a ponte Rust) built pras 4 arquiteturas
  Android, APK de 200MB gerado. Fecha P65. Ainda sem teste em emulador/hardware real (instalar o
  APK e escanear a câmera de fato) — lacuna menor, não bloqueia a fase
- [x] 9.8 — App desktop embute o próprio `warden-server` (Sessão 69, continuação — "virar o hub
  desta rede", pedido explícito do usuário) — até aqui o hub sempre foi um processo separado do
  desktop; agora a tela Workspace ganhou uma seção "Ser o hub desta rede" com toggle liga/desliga,
  porta escolhível (host fica fixo em `0.0.0.0`, já que o ponto é ser alcançável), auth key gerada
  automaticamente (nunca digitada — `warden_bootstrap::generate_auth_key`, 32 bytes aleatórios).
  Reaproveita o mesmo `Orchestrator` que já serve o chat local do desktop e os mesmos paths que o
  binário `warden-server` standalone já usaria (`default_server_conversations_dir`/
  `default_server_devices_path`), então `WorkspaceView.tsx`'s lista de dispositivos já pareados
  funciona sem nenhuma mudança. `Server` ganhou `serve_until` (shutdown gracioso — `serve()` sozinho
  nunca parava) pro toggle desligar de verdade e liberar a porta. Confirmado com o usuário: uma vez
  ligado, sobe sozinho em todo lançamento do app (`config.toml`'s `embedded_server.enabled`), não é
  um toggle só-desta-sessão. Verificado com um teste de ponta a ponta real (não mockado) — `Orchestrator`
  real via `bootstrap()`, `AppState` real, `Server::bind`/`serve_until` real, cliente real conectando
  e fazendo `Hello`+`Ping`+`Discover` de verdade sobre um socket TCP de verdade. Ver `ARCHITECTURE.md`
  pro detalhamento completo. Escolha explícita do usuário: escolher a porta pra acessar de fora da
  LAN (tipo Jellyfin) já funciona hoje sem nenhum código — é só redirecionar a porta no roteador —
  registrado como orientação, não feature, já que não depende do Warden

---

### Fase 10 — Autenticação & TruthID

**Objetivo**: Integrar login TruthID para autenticação e workspace de máquinas.

**Stack**: TruthID SDK, Rust, TypeScript

**Etapas**:
- [ ] 10.1 — Login via TruthID no app desktop
- [ ] 10.2 — Workspace: listar dispositivos pareados via Device Registry do TruthID
- [ ] 10.3 — Substituir auth local por TruthID (quando TruthID estiver em release estável)
- [ ] 10.4 — Deep link pareamento mobile↔desktop