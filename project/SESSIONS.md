# Log de Sessões

> **Nota**: Este log foi criado junto com o projeto. As sessões serão registradas aqui conforme o trabalho avança.
>
> Última atualização: 2026-08-09 (Sessão 33)

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