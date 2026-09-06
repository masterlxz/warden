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
- [x] 1.8 — Sub-agente leve: delegar tarefa escopada pra outro modelo/contexto
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

**Stack**: Rust, IPFS (Filebase/Pinata) — **superado, ver nota abaixo**

> **Nota (Sessão 50, 2026-09-06)**: a direção mudou de IPFS pra **Arweave via TruthID** — a
> carteira do TruthID paga/publica (não uma carteira própria do Warden), tudo cifrado antes de
> sair do device, escopo ampliado pra incluir `config.toml` inteiro (não só o vault), conversas
> ficam de fora. As etapas abaixo ainda descrevem o plano antigo (IPFS) e serão reescritas quando
> o desenho do manifesto de sync (diff tipo-git, ponteiro de "última versão") for fechado — ver
> P37 em `PENDING.md` e a entrada "Sync descentralizado (Fase 4)" em `ARCHITECTURE.md`. Primeira
> peça já implementada: `crates/warden-truthid` (cliente do protocolo `pin()` do TruthID).

**Etapas (plano antigo, será reescrito)**:
- [ ] 4.1 — Espelhar vault local em IPFS (pin via Filebase + Pinata)
- [ ] 4.2 — Cifra opcional do vault (AES-256-GCM, mesmo padrão TruthID Vault)
- [ ] 4.3 — Versionamento de memória (histórico de mudanças)
- [ ] 4.4 — Busca semântica no vault (embedding local ou via API)
- [ ] 4.5 — Backup automático em intervalo configurável
- [ ] 4.6 — Restore a partir de snapshot IPFS
- [ ] 4.7 — Configuração de providers de pinning

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
- [ ] 7.2 — Conectar ao servidor (Tailscale + WebSocket/gRPC)
- [ ] 7.3 — Interface de chat mobile *(a UI atual do desktop não serve como está — ver nota em
  `ARCHITECTURE.md`: sidebar de largura fixa praticamente toma a tela inteira num celular)*
- [ ] 7.4 — Execução de tools local (shell, arquivos)
- [ ] 7.5 — Notificações push
- [ ] 7.6 — Build e deploy

---

### Fase 8 — Extensão de Navegador

**Objetivo**: Extensão Chrome/Firefox que funciona como canal de chat + tool provider de browser.

**Stack**: Web Extension (Manifest V3), TypeScript

**Etapas**:
- [ ] 8.1 — Setup da extensão (Manifest V3, popup, background script)
- [ ] 8.2 — Canal de chat (popup com conversa)
- [ ] 8.3 — Tool provider: ler DOM da página ativa
- [ ] 8.4 — Tool provider: clicar em elementos
- [ ] 8.5 — Tool provider: navegar para URL
- [ ] 8.6 — Tool provider: extrair texto/seleção
- [ ] 8.7 — Comunicação com o servidor Warden (WebSocket)
- [ ] 8.8 — Publicação na Chrome Web Store / Firefox Add-ons

---

### Fase 9 — Rede de Nós & Tailscale

**Objetivo**: Múltiplos clientes conectados ao servidor, execução remota de tools.

**Stack**: Tailscale, WebSocket/gRPC, Rust

**Etapas**:
- [ ] 9.1 — Setup Tailscale (todos os nós na mesma subnet)
- [x] 9.2 — Protocolo servidor↔cliente — WebSocket + JSON próprio (handshake + heartbeat só;
  roteamento de tool pra 9.4/9.5), ver `ARCHITECTURE.md` e `PENDING.md`
- [ ] 9.3 — Registrar cliente no servidor (pareamento)
- [ ] 9.4 — Rotear requisição de tool para o cliente correto
- [ ] 9.5 — Cliente executa tool localmente e devolve resultado
- [ ] 9.6 — Workspace de máquinas (ver/gerenciar nós conectados)
- [ ] 9.7 — Pareamento de cliente novo via QR code (mesmo padrão TruthID)

---

### Fase 10 — Autenticação & TruthID

**Objetivo**: Integrar login TruthID para autenticação e workspace de máquinas.

**Stack**: TruthID SDK, Rust, TypeScript

**Etapas**:
- [ ] 10.1 — Login via TruthID no app desktop
- [ ] 10.2 — Workspace: listar dispositivos pareados via Device Registry do TruthID
- [ ] 10.3 — Substituir auth local por TruthID (quando TruthID estiver em release estável)
- [ ] 10.4 — Deep link pareamento mobile↔desktop