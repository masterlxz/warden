# Decisões de Arquitetura

## Registro de Decisões

| Decisão | Opções | Status |
|---|---|---|
| Framework desktop/mobile | Tauri vs Electron vs nativo | **Tauri** ✓ — reaproveita stack Rust/TS já usada no TruthID |
| Topologia de rede | Estrela vs Malha P2P | **Estrela** ✓ — servidor central, clientes se conectam. **Refinado 2026-08-02**: servidor é opcional, só entra quando a feature exige coordenação entre múltiplos nodes — ver nota abaixo e P14 em `PENDING.md` |
| Memória | Markdown vault (Obsidian) vs banco vetorial | **Markdown vault** ✓ — portátil, legível, versionável |
| Backup | IPFS (Filebase/Pinata) vs S3 vs auto | **IPFS** ✓ — mesmo padrão do TruthID Vault |
| Model-agnostic | Camada de abstração vs hardcoded | **Camada de abstração** ✓ — suporta OpenAI, Anthropic, Gemini |
| WhatsApp | Baileys (Node.js) vs nativo Rust | **Baileys (sidecar Node)** ✓ — não vale reescrever em Rust |
| Telegram | Bot API HTTP vs MTProto | **Bot API HTTP** ✓ — mais simples, sem risco de ban |
| Protocolo servidor↔cliente | gRPC vs WebSocket vs HTTP | **Em aberto** — candidatos: WebSocket (reaproveitar relay do TruthID) ou gRPC |
| Autenticação servidor↔cliente | Chave local vs TruthID | **Chave local no v1** ✓ — TruthID pode vir depois |
| Extensão de navegador | Canal + Tool provider vs só canal | **Canal + Tool provider** ✓ — expõe DOM/clique/navegação como tool MCP |
| Framework de CLI | clap vs structopt vs gum | **clap v4 (derive)** ✓ — structopt foi descontinuado e incorporado ao clap desde a v3; gum é ferramenta Bash/TUI, não se aplica a Rust |
| API de `web_search` | Tavily vs Brave Search API vs Google Custom Search | **Tavily** ✓ — feita pra tool use de agentes LLM (resultados já vêm resumidos), free tier de 1.000 buscas/mês sem cartão de crédito. Tool só é registrada se `TAVILY_API_KEY` estiver setada (degradação graciosa, mesmo espírito de P14) |
| Formato do arquivo de config | TOML vs YAML | **TOML** ✓ — convenção do próprio ecossistema Rust (mesmo formato do `Cargo.toml`), sem as ambiguidades clássicas de parsing do YAML, crate `toml` madura e serde-native. Localização default via crate `dirs` (`~/.config/warden/config.toml` no Linux, equivalente no Windows/macOS), com override via `--config`. Precedência: flag de CLI > variável de ambiente (só pra API keys) > arquivo de config > default embutido |
| Lógica de bootstrap compartilhada entre canais | Duplicar em cada canal vs crate à parte vs meter em `warden-core` | **Novo crate `crates/warden-bootstrap`** ✓ — carrega o config TOML e monta o `Orchestrator` (provider/vault/tools/sub-agente) uma vez só, usado por `warden-cli` e por `desktop/src-tauri`. Não entrou em `warden-core` de propósito (fonte de config é decisão de camada de aplicação/canal, não do motor model-agnostic); não virou "warden-cli como lib" pra não misturar parsing de CLI (clap) com algo que um app GUI também precisa. `default_vault_path` é parâmetro da função `bootstrap(...)` — cada canal informa o fallback certo pro seu contexto (CLI: `"vault"` relativo ao cwd; desktop: `~/Warden/vault`, já que o cwd de um app lançado por ícone é imprevisível) |
| Formato/local do histórico de conversas (6.6) | JSON no config dir vs dentro do vault markdown vs banco embutido (sqlite) | **JSON, um arquivo por conversa, em `~/.config/warden/conversations/<id>.json`** ✓ — mesmo `dirs::config_dir()` já usado pro `config.toml`, por ser dado opaco de app (não é conhecimento humano-navegável tipo o vault, que fica de propósito fora dessa pasta). Um arquivo por conversa (não um índice único) evita reescrever tudo a cada mensagem e isola corrupção — `list_conversations` já pula arquivo malformado em vez de falhar a lista inteira. Lógica em `warden-bootstrap` (`Conversation`/`ConversationMessage`/`save_conversation`/`list_conversations`), mesmo padrão de `save_config`/`load_config_from_path`; comandos Tauri (`list_conversations`/`save_conversation`) só chamam essas funções |
| Build/release do app desktop (6.8) | GitHub Actions (matrix por OS) vs cross-compilação local vs serviço de build de terceiro | **GitHub Actions, `.github/workflows/build.yml`** ✓ — replica quase literalmente o `build.yml` do TruthID (mesmo stack Tauri v2): dispara em tag `v*`, matrix `ubuntu-22.04`/`windows-latest`/`macos-latest`, `tauri-apps/tauri-action@v0` builda e publica um GitHub Release **draft** por tag. Sem assinatura de código nem notarização macOS (mesma decisão já tomada no TruthID — não bloqueante pro v1). Cross-compilação local nunca foi opção real: não tem SDK da Apple nem toolchain Windows nesta máquina de dev (Arch Linux) |
| Segurança da tool `shell` (5.5) | Sempre ativa (mesmo padrão de `read_file`/`write_file`) vs opt-in vs allowlist/sandboxing | **Opt-in, desligada por padrão** ✓ — decisão explícita do usuário, não da IA: uma tool de shell é categoricamente mais arriscada que as tools de arquivo (execução arbitrária de comando vs leitura/escrita já sem scoping real). Gate via `resolve_flag(WARDEN_ENABLE_SHELL, config.enable_shell)` (mesma precedência env > config do `resolve_secret`, mas pra booleano), com toggle também na tela de Settings do desktop (`enable_shell` em `SettingsSnapshot`/`SettingsFormPayload`) — religar/desligar já dá live-reload do orchestrator de graça, reaproveitando o mecanismo da 6.5. **Nenhum sandboxing/allowlist além disso** — uma vez ligada, o modelo pode rodar qualquer comando; as únicas proteções são `timeout_ms` (default 30s, capado em 5min, mata o processo via `kill_on_drop`) e truncamento de stdout/stderr (~20KB) pra não estourar o contexto. Execução via `sh -c` (Unix) / `cmd /C` (Windows) — PowerShell fica de fora por ora. `cwd` default = raiz do vault, overridável (relativo ou absoluto) |
| Registry de tools dinâmico (5.1) | Continuar só com `Vec<Arc<dyn Tool>>` fixo no `Orchestrator` vs um novo conceito de "fonte de tools descoberta em runtime" | **Trait `ToolProvider`** (`warden_core::tool::ToolProvider`, `async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>>`) ✓ — motivado pelo MCP client (5.2): conectar num server MCP não dá uma tool fixa, dá um conjunto de N tools só conhecido depois do handshake (`tools/list`). `Orchestrator::register_provider` chama `tools()` e registra cada uma via `register_tool` — é um snapshot no momento da chamada, sem live-sync se o server mudar seu conjunto de tools depois |
| MCP client (5.2) — SDK | Implementar o handshake JSON-RPC do MCP na mão vs usar SDK | **`rmcp`** ✓ (crates.io, mantido pela org `modelcontextprotocol`) — cobre `initialize`/`tools/list`/`tools/call` prontos; reimplementar seria retrabalho sem ganho, já que é um protocolo padronizado. Features habilitadas: `client` + `transport-child-process` em `warden-core` (produção); `transport-io` só em `[dev-dependencies]`, usado exclusivamente pelo teste real de subprocess (`tests/mcp_stdio.rs`) |
| MCP client (5.2) — transporte | stdio (child process) vs HTTP remoto | **stdio** ✓ para v1 — é o padrão de facto pra MCP local (mesmo shape `command`/`args`/`env` de qualquer client MCP, ex. Claude Desktop), cobre o caso concreto do usuário (Anchor/TruthID rodando local — P11/P13). `McpToolProvider::connect_stdio(name, command, args, env)` em `warden-core/src/tool/mcp.rs`; `env` é escopado só pro processo filho (`Command::envs`), nunca muta o processo do Warden. HTTP remoto fica pra quando surgir um caso real — `rmcp` já suporta, a interface (`ToolProvider`) não é stdio-specific |
| Config de servers MCP | Onde/como o usuário configura quais servers conectar | **`[[mcp_servers]]` no `config.toml`** ✓ (`FileConfig.mcp_servers: Vec<McpServerConfig>`, cada um com `name`/`command`/`args`/`env`) — mesmo formato que outros clients MCP usam, pra portar config existente quase verbatim. `bootstrap()` conecta em cada um e registra as tools; falha em conectar/listar não derruba o app, só loga um aviso e pula aquele server (mesmo espírito de degradação graciosa do Tavily/shell). **Sem UI ainda** — só editável a mão no config file; a tela de gerenciamento (adicionar/remover visualmente) é P11 em `PENDING.md`, ainda em aberto. Efeito colateral: `bootstrap()` virou `async fn` (precisa spawnar processo + aguardar handshake), o que exigiu `tauri::async_runtime::block_on` no `run()` do desktop (chamado antes do runtime do Tauri iniciar) e `save_settings` virar comando Tauri assíncrono |
| Tool `web_search` (5.3) | Manter REST hand-rolled (Tavily API direto, Fase 1.7) vs substituir pelo server MCP oficial da própria Tavily (`tavily-mcp`) | **Substituído por `tavily-mcp`** ✓ — decisão explícita do usuário (trade-off real, não óbvio): a versão REST era Rust puro, só exigia a API key; a versão MCP roda via `npx -y tavily-mcp`, então passa a exigir Node.js/`npx` no PATH em runtime pra essa capacidade. Aceito porque (a) a Tavily mantém o próprio server MCP oficial (https://docs.tavily.com/documentation/mcp), então não é uma integração de terceiro frágil; (b) dá busca **e** extract/crawl/map/research de graça (5 tools, confirmado rodando contra o server real, não só o de teste da 5.2); (c) serve de validação de ponta a ponta do client MCP contra um server publicado de verdade. `WebSearchTool` (`tool/web_search.rs`) foi removido — o gate por `TAVILY_API_KEY` continua igual (`resolve_secret`), só que agora chama o novo helper `register_mcp_server_tools` em vez de construir a tool direto. Esse helper também passou a ser reaproveitado pelo loop de `config.mcp_servers`, que tinha exatamente o mesmo padrão connect→list→extend duplicado |
| Tool `file_system` (5.6) — escopo | Substituir `read_file`/`write_file` (vault) vs capacidade nova e separada, fora do vault | **Capacidade nova e separada** ✓ — decisão explícita do usuário. `read_file`/`write_file` continuam só pro vault (uso interno de memória); `file_system` é acesso a diretórios arbitrários do sistema, no espírito do `shell` (5.5) mas pra I/O de arquivo em vez de execução de comando. Motivado em parte por uma lacuna real encontrada em `memory/mod.rs`: `Vault::read`/`write` fazem só `root.join(path)`, sem canonicalizar nem conter — path traversal não é bloqueado hoje nos tools do vault |
| Tool `file_system` (5.6) — implementação | Campo de config dedicado (`file_system_allowed_dirs`) vs reusar o mecanismo genérico `[[mcp_servers]]` da 5.2 | **Reusar `[[mcp_servers]]`, sem campo novo** ✓ — diferente do Tavily (que ganhou tratamento dedicado por ter um "segredo" único e óbvio, `TAVILY_API_KEY`, já com campo na Settings UI), `file_system` não tem equivalente natural pra virar config especial: é só uma lista de diretórios arbitrários, exatamente o que `mcp_servers` já resolve de forma genérica. Um campo bespoke duplicaria o mecanismo sem adicionar capacidade — habilita-se via: `[[mcp_servers]] name = "file_system" command = "npx" args = ["-y", "@modelcontextprotocol/server-filesystem", "/caminho/permitido"]`. **Sem UI ainda** — mesma decisão do usuário já tomada pra `mcp_servers` em geral (P11 em `PENDING.md`). Verificado de ponta a ponta contra o server oficial de verdade (não mockado): 14 tools listadas (`read_file`, `read_text_file`, `write_file`, `edit_file`, `move_file`, `search_files`, etc.), round-trip real de escrita+leitura dentro do diretório permitido funcionou, e uma tentativa de escrever **fora** do diretório permitido foi rejeitada pelo próprio server (`"Access denied - path outside allowed directories"`, confirmado que o arquivo não foi criado em disco) — prova concreta de que essa capacidade é mais segura que o `read_file`/`write_file` atual do vault |
| Integração Google (5.7) — qual server MCP | O server oficial de referência (`@modelcontextprotocol/server-gdrive`) está **arquivado** (`servers-archived`, sem manutenção) — não existe mais opção oficial. Candidatos da comunidade pesquisados: `taylorwilsdon/google_workspace_mcp` (uvx/Python, 2987★, o mais completo — 120+ tools em 12 serviços) vs `aaronsb/google-workspace-mcp` (npx/Node.js, 164★, mantido ativamente, 11 tools em 7 serviços incl. multi-conta nativo) vs outros com pouca tração (`dguido` **arquivado**, `danielrosehill` 1★, `j3k0` 32★) | **`aaronsb/google-workspace-mcp` via `npx`** ✓ — decisão explícita do usuário: prevaleceu consistência de runtime sobre cobertura de tools. O projeto já aceitou Node.js/`npx` como dependência de runtime pro Tavily (5.3) e pro `file_system` (5.6) — ver P17 em `PENDING.md`; escolher o candidato uvx/Python teria introduzido um **segundo** runtime obrigatório (`uv`) só pra essa capacidade. `aaronsb` cobre exatamente o escopo do PHASE.md (Gmail/Calendar/Drive) mais Sheets/Docs/Tasks/Meet de bônus, com suporte a múltiplas contas Google já embutido (`manage_accounts`) |
| Integração Google (5.7) — implementação | Igual à 5.6: campo de config dedicado vs reusar `[[mcp_servers]]` | **Reusar `[[mcp_servers]]`, sem campo novo** ✓ — mesmo raciocínio da 5.6, credenciais são só duas env vars (`GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`), sem "segredo único e óbvio" que justifique um campo bespoke tipo o do Tavily. Habilita-se via: `[[mcp_servers]] name = "google_workspace" command = "npx" args = ["-y", "@aaronsb/google-workspace-mcp"] [mcp_servers.env] GOOGLE_CLIENT_ID = "..." GOOGLE_CLIENT_SECRET = "..."`. **Setup fora do Warden, feito pelo usuário**: criar um projeto no Google Cloud Console, habilitar as APIs desejadas (Gmail/Calendar/Drive/...), criar credencial OAuth 2.0 tipo "Desktop app" — não tem como automatizar isso do lado do Warden. No primeiro uso, a tool `manage_accounts` (operação `authenticate`) abre um browser pro consent OAuth; token fica salvo em `~/.local/share/google-workspace-mcp/credentials/`, por conta. Verificado de ponta a ponta contra o server real via `McpToolProvider::connect_stdio` (mesmo caminho de produção), **sem** credenciais setadas: conecta normalmente (server não exige env vars no handshake) e lista os 11 tools reais (`manage_email`, `manage_calendar`, `manage_drive`, `manage_docs`, `manage_sheets`, `manage_tasks`, `manage_meet`, `manage_accounts`, etc.) — confirma que a integração ponta a ponta funciona sem nenhum código novo; só falta o usuário configurar credenciais próprias pra usar de verdade |
| Rate limiting/custo por tool (5.8) — escopo | PHASE.md/P4 pediam "rate limiting e controle de custo por tool" — dois mecanismos distintos (limitar frequência vs rastrear/limitar gasto) | **Só tracking de uso** ✓ — decisão explícita do usuário, entre 4 opções apresentadas (tracking só; tracking + teto rígido de custo; rate limiting por tool; os dois). Captura e persiste os tokens que cada chamada de modelo já reporta (hoje 100% descartados — `grep` por `usage`/`cost`/`rate_limit`/`token` não batia em nada no repo inteiro), sem nenhum limite/bloqueio ainda. Base mínima pro dashboard de custo futuro (P10); rate limiting de verdade e teto de gasto configurável continuam em aberto, P4 estreitada pra cobrir só essa parte restante |
| Rate limiting/custo por tool (5.8) — implementação | Onde capturar e como propagar o uso até a persistência | **Novo tipo `Usage` (`warden_core::model`)** com `prompt_tokens`/`completion_tokens`/`total_tokens`, populado por cada provider a partir do que a própria API já devolve (`usage` da OpenAI, `usageMetadata` do Gemini — nomes de campo estáveis e documentados publicamente, não um mecanismo de terceiro a validar como na 5.3/5.6/5.7). `Response` ganha `usage: Option<Usage>`; `Orchestrator::handle_message` (único chokepoint por onde toda chamada de modelo passa) some o uso das até `MAX_TOOL_ITERATIONS` chamadas de uma mesma mensagem e devolve num novo tipo `MessageOutcome { content, usage }` no lugar do `String` que devolvia antes. `ConversationMessage` (bootstrap) ganha `#[serde(default)] usage: Option<Usage>` — `#[serde(default)]` é o que mantém conversas já salvas em disco (sem esse campo) carregando sem erro. Desktop: `send_message` passa a devolver `{content, usage}` em vez de `String` cru; frontend anexa um badge discreto de tokens só na mensagem do assistente (`MessageBubble.tsx`, `var(--color-text-muted)` + `0.82em`, mesma convenção já usada em outros textos secundários da UI — não havia nenhum metadado por mensagem renderizado antes disso). CLI: sem persistência de conversa nenhuma hoje, então é só um echo de visibilidade após a resposta, não storage. **Gap aceito e documentado no código** (`tool/delegate.rs`): o uso do sub-agente disparado por `DelegateTool` não sobe pro total da conversa pai — `Tool::call` só devolve `serde_json::Value`, não `MessageOutcome`; fechar isso exigiria mudar a trait `Tool` inteira, fora do escopo mínimo desta sessão |
| Canal Telegram (Fase 2) — trait `Channel` | `PHASE.md` (etapa 2.2) cita uma trait `Channel`, mas nem `warden-cli` nem o desktop compartilham hoje nenhuma abstração além de chamar `bootstrap()` uma vez cada — investigação não achou nenhuma trait `Channel` existente | **Sem trait ainda** ✓ — decisão explícita do usuário. Em vez disso, `warden-bootstrap` ganhou uma função concreta e reutilizável, `handle_turn(orchestrator, conversations_dir, conversation_id, title_seed, user_input) -> anyhow::Result<MessageOutcome>`: carrega a conversa por id (novo `load_conversation`, a versão "uma conversa só" do `list_conversations` existente), chama `orchestrator.handle_message`, acrescenta as duas mensagens novas e persiste. `crates/warden-telegram` usa isso; `warden-cli`/desktop continuam como estavam (CLI não persiste nada; desktop já faz seu próprio read/append/save no frontend). Motivo: uma trait com um único implementador não valida a forma certa da abstração — fica pra quando o WhatsApp (Fase 3) existir e dois exemplos reais mostrarem o que é de fato compartilhável (bot HTTP long-polling vs sidecar Node+IPC do Baileys são bem diferentes) |
| Canal Telegram (Fase 2) — cliente HTTP testável | Como testar o loop de receive→rotear→responder sem depender da API real do Telegram — o repo não tem nenhuma crate de mock HTTP (`wiremock` etc.) | **Trait fina `TelegramApi`** (`get_updates`/`send_message`) implementada de verdade por `TelegramClient` e mockada em teste por `ScriptedTelegramApi` — mesmo espírito do `ModelProvider`/`ScriptedModel` já usado em `warden-core/tests/pipeline.rs`, sem introduzir dependência nova só pra isso. `run_bot`/`process_updates`/`handle_update` (`crates/warden-telegram/src/telegram.rs`) são genéricos sobre `impl TelegramApi`, então os testes hermáticos rodam o loop de verdade (offset avança, `/start`/`/help` não chama o orchestrator, conversa persiste com as 2 mensagens) sem nenhuma chamada de rede |
| Canal Telegram (Fase 2) — formatação das respostas (etapa 2.5) | Texto puro vs MarkdownV2 real | **Texto puro (sem `parse_mode`)** ✓ — decisão explícita do usuário. O MarkdownV2 do Telegram tem sintaxe própria (diferente de CommonMark) e a API rejeita a mensagem inteira se um caractere reservado não escapado aparecer no texto — um conversor de verdade (CommonMark do modelo → MarkdownV2) é trabalho substancial, não só escapar caracteres. Fica pra uma sessão futura se formatação (negrito/listas/code block) fizer falta na prática |
| Canal Telegram (Fase 2) — diretório de conversas | Reusar `default_conversations_dir()` (o que o desktop lista no sidebar) vs um diretório próprio | **Diretório próprio**, `default_telegram_conversations_dir()` (`~/.config/warden/conversations-telegram/`) ✓ — decisão do agente, não perguntada (baixo risco, reversível): conversas do Telegram são tituladas por `chat_id`/username, sem client-side UI nenhuma, e misturá-las no mesmo diretório faria o sidebar do desktop listar entradas que ele não sabe rotular direito. Uma visão unificada de conversas entre canais é uma pergunta de produto maior, registrada como nota de `ROADMAP.md`, não resolvida aqui |
| Canal Telegram (Fase 2) — transporte | Long polling (`getUpdates`) vs webhook | **Long polling** ✓ — já antecipado no mapa de dependência de servidor deste arquivo ("Canal Telegram (long polling, Fase 2)"), consistente com o princípio geral do projeto de não exigir papel de servidor pra canais (P14 em `PENDING.md`): um processo sempre ligado no próprio device do usuário basta, sem precisar expor um endpoint HTTPS público que um webhook exigiria |
| Canal WhatsApp (Fase 3) — IPC sidecar↔core | `PHASE.md` deixava em aberto ("stdin/stdout ou socket"). Não existia nenhum precedente de socket/IPC customizado no repo — só o `TokioChildProcess` do client MCP (`crates/warden-core/src/tool/mcp.rs`), inteiramente específico do protocolo MCP (JSON-RPC via `rmcp`), não reaproveitável como transporte genérico | **stdin/stdout, JSON-lines** ✓ — decisão explícita do usuário. Reusa o mesmo padrão de spawn já usado pro MCP (`tokio::process::Command`), só com um protocolo JSON simples do próprio projeto em vez do MCP. Evita ser a primeira dependência de `tokio::net` do projeto, gerenciamento de arquivo de socket, e a diferença Unix-socket vs named-pipe do Windows. `crates/warden-whatsapp/src/sidecar.rs`: trait `WhatsAppSidecar` (`&mut self` — ao contrário da `TelegramApi` da Fase 2, que é `&self`, porque ler linha a linha de um stream é estado, não uma chamada nova a cada vez), `ChildSidecar` implementa de verdade sobre `ChildStdin`/`ChildStdout` via `tokio::io::{AsyncBufReadExt, AsyncWriteExt}` (nova feature `io-util` do tokio, só nesta crate); `ScriptedSidecar` mocka em teste, mesmo espírito da `ScriptedTelegramApi` |
| Canal WhatsApp (Fase 3) — protocolo IPC | Forma exata das mensagens JSON entre sidecar e core | Uma linha JSON por evento/comando. Sidecar → Rust (stdout): `{"type":"connected"}`, `{"type":"disconnected","loggedOut":bool}`, `{"type":"message","chatId":...,"senderName":...,"text":...\|null}` (`text: null` = mensagem sem corpo de texto legível, imagem/áudio/documento/etc. — é o que aciona a degradação graciosa da etapa 3.7). Rust → sidecar (stdin): `{"type":"send","chatId":...,"text":...}`. **QR code não entra no protocolo** — vira um arquivo PNG, caminho logado no **stderr** do sidecar (canal separado do stdin/stdout usado pro IPC), Rust só herda esse stderr (`Stdio::inherit()`). Ver decisão específica de renderização do QR abaixo |
| Canal WhatsApp (Fase 3) — código JS no repo | Onde/como versionar o script Node que fala com o Baileys — primeira vez que o projeto **escreve e versiona** código JS (tudo antes era `npx` contra pacotes de terceiros: Tavily, filesystem, Google Workspace) | `sidecar/whatsapp/` na raiz do repo (irmão de `desktop/`, outro subtree não-Rust com `package.json` próprio) — `package.json` + `index.mjs` puro (ESM, sem TypeScript/build step, já que é só cola fina em cima do Baileys). `baileys@^7.0.0-rc14` — a linha estável `6.7.24` (`dist-tags.legacy` no npm) era a intenção original, mas seu `libsignal` é resolvido via `git+https://...` em vez do registry npm, e o ambiente de verificação bloqueia fetch de dependências git; `7.0.0-rc14` resolve `libsignal` como dependência normal do registry e instalou/conectou de verdade. Trade-off aceito conscientemente: é uma pre-release pré-1.0, não a tag `latest`-estável — revisitar se uma `7.x` de verdade sair ou se `6.x` passar a instalar no ambiente do usuário. **Setup manual, sem automatizar**: `npm install` dentro de `sidecar/whatsapp/` antes do primeiro uso — mesmo espírito de não auto-instalar dependências de runtime já usado pro Node/npx em geral (P17). Verificado de ponta a ponta contra os servidores reais do WhatsApp (não mockado, e depois confirmado de novo pelo usuário na própria máquina): `warden-whatsapp` bootstrapa, spawna o sidecar de verdade, o sidecar conecta e gera um QR de pareamento real |
| Canal WhatsApp (Fase 3) — renderização do QR | `qrcode-terminal` (ASCII no terminal) vs `qrcode` (arquivo PNG) | **PNG** ✓ — decisão do agente, revisada depois de feedback real do usuário: a primeira versão usava `qrcode-terminal` com o truque de meio-bloco Unicode pra "comprimir" o QR verticalmente, assumindo uma proporção de fonte de terminal específica (~2:1 altura:largura); quando essa suposição não bate o QR sai visivelmente esticado — uma câmera genérica tolera a distorção, o scanner do próprio WhatsApp não. Trocado por `qrcode` (`QRCode.toFile`), que gera um PNG de verdade (512×512, pixels quadrados, sem depender de fonte nenhuma) salvo em `<authDir>/qr.png`, com o caminho logado no stderr do sidecar. Verificado de ponta a ponta: PNG válido gerado (`file` confirma 512×512 RGBA), stdout continua limpo (0 bytes) |
| Canal WhatsApp (Fase 3) — tratamento de mídia (etapa 3.7) | Degradação graciosa vs suporte multimodal de verdade | **Só degradação graciosa** ✓ — decisão explícita do usuário. Suporte de verdade (o modelo "entender" imagem/áudio) exigiria mudar `ModelProvider`/`Message` (`warden-core`) pra multimodal — mudança de model-layer que toca os dois providers (OpenAI, Gemini), não uma mudança de canal. Registrada como pendência nova (P21 em `PENDING.md`) pra uma sessão à parte |
| Canal WhatsApp (Fase 3) — trait `Channel` (fecha P20) | Com dois canais reais agora (Telegram HTTP long-polling, WhatsApp sidecar Node+IPC), P20 perguntava se valia revisitar a trait `Channel` adiada na Fase 2 | **Continua sem trait** — o que de fato é compartilhável entre os dois já está extraído em `handle_turn` (`warden-bootstrap`); os loops de recebimento em si (`TelegramApi::get_updates` via poll HTTP com offset vs `WhatsAppSidecar::recv_event` via stream de eventos sobre stdio, `&self` vs `&mut self`) continuam genuinamente diferentes o bastante pra uma trait `Channel` não reduzir duplicação real — só forçaria os dois loops numa assinatura comum sem corpo compartilhado. P20 fechada com essa conclusão; reabrir se um terceiro canal mostrar um padrão diferente |

---

## Topologia: Estrela (Servidor + Clientes)

```
┌─────────────┐
│  Servidor   │ (homelab/desktop fixo)
│  - Modelo   │
│  - Memória  │
│  - Tools    │
└──────┬──────┘
       │ Tailscale
       ├──────────────────┐
┌──────┴──────┐    ┌──────┴──────┐
│  Cliente    │    │  Cliente    │
│  Desktop    │    │  Mobile     │
└─────────────┘    └─────────────┘

┌─────────────┐    ┌─────────────┐
│  Extensão   │    │  Telegram   │
│  Browser    │    │  WhatsApp   │
└─────────────┘    └─────────────┘
```

### Servidor é opcional (refinamento 2026-08-02)

O diagrama acima é o caso de **múltiplos nodes**. Mas um único node (ex: só o
app rodando no celular do usuário, sem nenhum outro dispositivo) deve
funcionar **100% standalone, sem nenhum servidor central** — cliente e
servidor colapsam no mesmo processo local.

Princípio: o servidor só existe pra resolver o que **de fato** exige
coordenação entre múltiplos nodes. Fora isso, não é necessário.

Reframe importante: "servidor" aqui **não é uma categoria de infraestrutura
à parte**. É sempre um device que o próprio usuário já tem (o desktop fixo, o
homelab) só que designado como "o que fica sempre ligado". Não existe nenhum
cenário no Warden que exige infraestrutura de terceiro que o usuário não
controle — o pior caso é "preciso que uma das minhas próprias máquinas
fique ligada", nunca "preciso alugar/operar um serviço".

### Mapa de dependência de servidor (P14)

| Precisa de servidor? | Feature |
|---|---|
| ❌ Não | Orquestrador, model calls, tools, vault local (Fase 1) |
| ❌ Não | Canal Terminal (Fase 1.3 / canal Terminal completo, P8) |
| ❌ Não | App Desktop ou Mobile rodando sozinho, 1 device (Fases 6/7) |
| ❌ Não | Warden como *client* MCP — conectar em servers externos, ex. Anchor (P11a) |
| ❌ Não | Sub-agente leve — invocação síncrona, mesmo processo (1.8) |
| ❌ Não | Backup em IPFS — usa Filebase/Pinata, não infra própria (Fase 4) |
| ❌ Não | Warden API (P12) ou Warden como *server* MCP (P11b), **se só chamado do mesmo device** (localhost) |
| ⚠️ Precisa de "algo sempre ligado" — não é topologia servidor↔cliente, é só uptime | Canal Telegram (long polling, Fase 2) |
| ⚠️ Precisa de "algo sempre ligado" | Canal WhatsApp (sessão Baileys tem que ficar conectada, Fase 3) |
| ⚠️ Precisa ser alcançável de fora do device | Warden API (P12) chamada de fora do device onde o Warden roda |
| ⚠️ Precisa ser alcançável de fora do device | Warden como *server* MCP (P11b) pra app remoto/cloud, não local |
| ✅ Sim — precisa do node primário (papel "servidor" de fato) | Execução remota de tool em outro device (Fase 9) |
| ✅ Sim | Pareamento / workspace de múltiplos devices (Fase 9.6/9.7) |
| ✅ Sim | Extensão de navegador falando com orquestrador rodando em **outro** device (se for o mesmo device, cai no ❌) |

Conclusão: só a Fase 9 (rede de nós) exige de fato o papel "servidor" da
topologia estrela. Tudo mais é local por padrão, ou vira servidor só na
medida em que o usuário decide expor pra fora do próprio device — nunca por
exigência estrutural. O protocolo servidor↔cliente (P1) só importa pra essa
fatia "✅ Sim".

P14 fica marcado como respondido por esse mapeamento — pode ser revisado
quando a Fase 9 for implementada de verdade e detalhes concretos aparecerem.

---

## Model-Agnostic: Como Funciona

O orquestrador não sabe qual modelo está rodando. Ele fala com uma trait/interface comum:

```rust
trait ModelProvider {
    fn chat(&self, messages: Vec<Message>, tools: Vec<Tool>) -> Result<Response>;
}
```

Cada provedor implementa essa trait:
- `OpenAIProvider` — API da OpenAI
- `AnthropicProvider` — API do Claude
- `GeminiProvider` — API do Google
- `LocalProvider` — modelo rodando local (ollama, llama.cpp)

---

## Canais como Adapter

Cada canal implementa:

```rust
trait Channel {
    fn send(&self, message: Message) -> Result<()>;
    fn receive(&self) -> Result<Message>;
}
```

O orquestrador não sabe de onde veio a mensagem — só processa e responde.

---

## Memória: Vault Markdown

- Arquivos `.md` no disco local do servidor
- Estrutura de pastas livre (o usuário organiza como quiser)
- Espelhado em IPFS (Filebase + Pinata)
- Busca full-text via grep/ripgrep (simples, sem precisar de banco vetorial no v1)

---

## Sub-agentes: Invocação Leve vs. Autônomos

- **Invocação leve (v1)**: agente principal chama sub-agente escopado pra tarefa específica, contexto reduzido, devolve resultado, encerra. Implementado como `DelegateTool` (`crates/warden-core/src/tool/delegate.rs`, tool `delegate_task`) — internamente é só mais um `Orchestrator` completo (mesmo `model`, mesmo `vault`, subconjunto de tools escolhido pelo chamador, nunca incluindo outro `DelegateTool`), reaproveitando 100% do loop de tool-calling existente em vez de duplicar lógica
- **Sub-agentes autônomos (fora de escopo v1)**: criam outros agentes recursivamente, precisam de fila de jobs, controle de custo, isolamento — fica pra depois