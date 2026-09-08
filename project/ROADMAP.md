# Roadmap e Evoluções Planejadas

## Sequenciamento Sugerido

> **Atualizado em 2026-08-29 (Sessão 35)** — nova re-priorização do usuário, por cima da de
> 2026-08-02 abaixo. Objetivo explícito: "fazer isso aqui ser o melhor agente pessoal possível".
> Ordem pedida: (1) polir o **App Desktop** agora — UX geral, gerenciamento de API keys
> (salvar/apagar) e suporte a múltiplos provedores (Ollama local + as principais empresas que
> ainda faltam, ex. Anthropic — hoje só Gemini/OpenAI existem, ver `ARCHITECTURE.md`); (2) voltar
> a expandir **Tools & MCP** (Fase 5) — maximizar quantas coisas o agente consegue acessar via
> servers MCP; (3) **App Mobile** (Fase 7); (4) depois disso, os caminhos que ficaram pra trás:
> **Canal Terminal/CLI** (melhorar a UX, que o usuário ainda considera "feia" mesmo depois do
> polish da Sessão 31/33 — ver P8 em `PENDING.md`), desenhar um **app servidor** de verdade (Fase
> 9), e **Vault & Memória** (Fase 4) — que muda de figura: o usuário já não quer IPFS, e sim
> **Arweave**, seguindo o pivô que o próprio TruthID já fez (confirmado nesta sessão:
> `docs/docs/sdk/dart.md` do TruthID usa carteira Arweave por identidade e ponteiro `ar://`, não
> mais CID IPFS — vale estudar a fundo a arquitetura de vault do TruthID antes de desenhar a do
> Warden, ver pendência nova em `PENDING.md`); (5) por fim, **conexão com o TruthID** (Fase 10,
> autenticação unificada) segue confirmada como importante. Os números das fases em `PHASE.md`
> não mudaram — só a ordem de execução abaixo.

> **Atualizado em 2026-08-02** — decisão do usuário: priorizar o App Desktop (Fase 6) logo
> depois da Fase 1, antes dos canais de mensageria (Telegram/WhatsApp) e do resto da Fase
> 4/5. Motivo: interface de chat de verdade importa mais agora do que canais externos, e
> tecnicamente não há bloqueio — o app desktop só faz IPC local Rust↔frontend (não depende da
> topologia servidor↔cliente, que só entra na Fase 9; ver P14 em `PENDING.md`). Os números das
> fases em `PHASE.md` não mudaram — só a ordem de execução abaixo.

Ordem de implementação recomendada (atualizada 2026-08-29, ver nota acima):

1. **Orquestrador CLI + 1 modelo + vault local** (Fase 1) — base de tudo ✅ concluída
2. **App Desktop** (Fase 6) — interface nativa de chat ✅ concluída — **agora em polish**: UX,
   gerenciamento de API keys (CRUD de verdade, não só campo de texto), múltiplos provedores
   (Ollama, Anthropic, outros)
3. **Canal Telegram** (Fase 2) ✅ concluída
4. **Canal WhatsApp** (Fase 3) ✅ concluída
5. **Tools & MCP** (Fase 5) — quase completa (falta só 5.4, bloqueada pela Fase 8) — **próximo
   foco depois do Desktop**: maximizar integrações via servers MCP existentes
6. **App Mobile** (Fase 7) — cliente móvel, depois do MCP
7. **Canal Terminal/CLI** — revisitar UX (P8), usuário ainda insatisfeito
8. **Rede de nós + Tailscale / app servidor** (Fase 9) — execução remota de tools
9. **Vault & Memória** (Fase 4) — repensar com Arweave em vez de IPFS, estudando a arquitetura
   real do TruthID primeiro
10. **Extensão de Navegador** (Fase 8) — canal + tool de browser
11. **Integração TruthID** (Fase 10) — autenticação unificada, confirmada como importante

---

## Ideias de Expansão (Brainstorm — sem `/plan`)

> **Sessão 53 (2026-09-06)** — leva grande de ideias novas do usuário, dadas de uma vez, cru
> ("são apenas ideias pro projeto"), sem `/plan` e sem decisão de prioridade ainda além do que
> está anotado em cada uma. Ver P45-P52 em `PENDING.md`.

### ~~Um agente por conversa (seleção no início)~~ — feito (Sessão 57)

Implementado — ver `PENDING.md` P45 (resolvida) e `ARCHITECTURE.md`. Escopo: só desktop.

### Orquestração de agentes — dois modos de uso

Detalhado pelo usuário (2026-09-06), evoluindo a ideia de "Sub-agentes autônomos" logo abaixo:
quer atender dois perfis de usuário diferentes, não escolher um só —

- **Modo centralizado**: um agente único (o "chefe") que cria e comanda outros agentes, mas o
  usuário só interage com o chefe — os sub-agentes ficam geridos por trás
- **Modo "funcionários"**: vários agentes especializados e independentes, sem nenhum agente
  global coordenando — o usuário fala com cada um diretamente, cada um no seu escopo

Em ambos os modos, agentes devem poder criar outros agentes (não só um orquestrador raiz fixo).
Arquitetura pra suportar os dois modos ao mesmo tempo (não é escolher um dos dois) ainda em
aberto. Ver P46, complementa P8/a seção "Sub-agentes autônomos" abaixo.

### SSH — conectar com VPS e máquinas externas

Ideia nova (2026-09-06): cadastrar chaves SSH nas configurações do Warden, dar (ou negar) à IA
acesso a cada uma, e a IA poder usar isso pra rodar comandos em servidores/VPS remotos — não só
a máquina local (tool `shell`, Fase 5.5). Usuário também quer que a IA possa **criar** esse tipo
de infraestrutura — escopo exato (provisionar um VPS do zero? só conectar num já existente?)
ainda não detalhado. Ver P47.

### Avatares/personas para agentes (3D, animações)

Ideia nova (2026-09-06), bem mais ambiciosa: dar um "personagem" visual a um agente — ex. um
gatinho — gerado em 3D com animações, criável de duas formas: (a) só um prompt de texto, ou (b)
uma foto + prompt (ou só a foto). O avatar poderia "se mexer no computador" (overlay animado na
tela, não só um ícone estático). Nada de arquitetura definida ainda — geração 3D a partir de
texto/imagem é um problema de pesquisa em si (que modelo/serviço gera isso? rigging de animação
automático?). Ver P48.

### Overlay "Super Jarvis" — atalho global + avatar na tela

Ideia nova (2026-09-06), evolução do app "Copilot" logo abaixo (P9): atalho de teclado global
pra ativar áudio ou abrir uma tela de busca/chat da IA **sem precisar abrir o app** — nessa tela
o avatar (ideia acima) aparece, e dá pra conversar por texto ou voz. Visão declarada pelo
usuário como "um super Jarvis mesmo". Ver P49, complementa P9.

### Tier pago — hospedagem do servidor pelo próprio Warden

Ideia nova de modelo de negócio (2026-09-06): hoje a Fase 9 (rede de nós/servidor) pressupõe o
usuário hospedando o próprio servidor (VPS ou em casa). Um tier pago ofereceria hospedar esse
servidor pelo próprio Warden (SaaS), pra quem não quiser cuidar de infraestrutura própria.
Primeira menção de um modelo de negócio pago no projeto. Ver P50.

### "9Router" — API do agente pessoal + OAuth de contas de IA

Evolução concreta da ideia "Warden API" abaixo (P12): o usuário quer nomear e detalhar isso como
**"9Router"** — uma API completa que expõe o agente pessoal dele (conhecimento do vault +
personalidade configurada) pra ser usada em **outros harnesses**, não só dentro do Warden.
Inclui conectar contas via **OAuth de provedores de IA** (Claude, GPT, e outros) —
presumivelmente pra permitir usar a conta/assinatura que o usuário já paga em vez de (ou além
de) uma chave de API bruta. Usuário sinalizou que "vem a hora" de puxar isso pra frente. Ver
P51, substitui/evolui P12.

### Estrutura padrão do vault + visualização pela interface

Ideia nova (2026-09-06) pra Fase 4 (Vault & Memória): dentro da memória compartilhada entre
agentes, o usuário quer uma parte **fixa/padrão** (perfil do usuário, comportamento da IA) e o
resto **livre**, a critério da própria IA organizar (ex. arquivo sobre o cachorro, família,
estudos). Também quer que o vault seja bem organizado e legível — não só pra IA, mas de fácil
visualização **pela interface** (não só arquivos markdown crus). Ver P52, relacionado a P37
(que cobre "como sincronizar" via Arweave, não "como estruturar").

### Canal Terminal (estilo Claude Code)

Confirmado pelo usuário (2026-08-02): terminal como canal completo de conversa,
não só o loop de bootstrap da Fase 1.3. Interativo, no espírito do Claude Code,
mas **não é assistente de programação** — o foco é produtividade geral de
comandos de terminal (automatizar, explicar, compor comandos do dia a dia).

- Cross-platform: Linux, Windows, Mac (shells diferentes — bash/zsh vs
  PowerShell/cmd — importa pra tool `shell` da Fase 5.5, não só pro canal)
- A expertise de terminal não é exclusiva do canal Terminal: o usuário quer que
  o agente saiba/possa executar comandos **a partir de qualquer canal**
  (Telegram, WhatsApp, desktop). O canal Terminal é só a UI mais natural pra
  isso, a capacidade em si é a tool `shell` (Fase 5.5) exposta globalmente
- Provável evolução do loop simples stdin/stdout (Fase 1.3) pra algo mais rico
  (histórico, autocomplete, talvez TUI com `ratatui`) — ver P8 em `PENDING.md`

### Visão unificada de conversas entre canais

Levantado durante a implementação do canal Telegram (Fase 2, sessão 2026-08-09): hoje cada
canal que persiste conversas usa seu próprio diretório (desktop em `~/.config/warden/
conversations/`, Telegram em `~/.config/warden/conversations-telegram/`, ver `ARCHITECTURE.md`)
— evita misturar dado sem título humano-legível (chat_id do Telegram) no sidebar do desktop, mas
significa que "a mesma conversa" não existe entre canais: falar com o Warden pelo Telegram e
depois abrir o desktop não mostra o mesmo histórico. Se isso incomodar na prática, vale desenhar
uma visão de conversas realmente unificada entre canais — não resolvido ainda, é uma pergunta de
produto maior que a Fase 2 não tentou responder.

### Integração com Discord

Confirmado pelo usuário (2026-08-31, Sessão 38): quer conectar com o Discord, sem urgência —
"acho que vai ser útil no futuro". Registrado só como visão por enquanto, nenhuma implementação
começada. Duas frentes independentes (ver P27 em `PENDING.md`), mesmo padrão já usado pro Slack:

- **Warden como bot no Discord** — canal novo no espírito do Telegram/WhatsApp (Fases 2/3), mas
  nenhum dos dois padrões existentes bate certinho: não é polling HTTP com offset (Telegram) nem
  um sidecar Node/Baileys por WebSocket próprio (WhatsApp) — a Discord API usa seu próprio
  protocolo de gateway (WebSocket com heartbeat/intents). Provavelmente um crate novo
  (`warden-discord`?), histórico de conversa por canal/DM, mesma função `handle_turn`
  reaproveitada do lado da lógica de conversa (ver P20 em `PENDING.md` — decisão já tomada de não
  ter uma trait `Channel`, cada canal com seu próprio loop de recebimento)
- **Discord como MCP server** — bem mais simples de ligar: mesmo mecanismo de preset "quick add"
  já usado pro Slack/Notion/GitHub (`[[mcp_servers]]`, Fase 5.2/P11), plugando um MCP server de
  Discord já existente no mercado (a checar qual — provavelmente `npx`-based, mesma família do
  resto dos presets). O agente ganha acesso a ler/postar mensagens sob demanda, sem código novo
  no Warden além de um preset a mais na UI

Quando isso for retomado, vale perguntar ao usuário qual das duas frentes puxar primeiro — o
próprio usuário já sinalizou (Sessão 38) que o MCP é o caminho mais rápido de ligar hoje.

### Sub-agentes autônomos

Agentes que criam outros agentes recursivamente para tarefas complexas.
Usuário confirmou interesse em "lançar agentes" (2026-08-02) — arquitetura
ainda em aberto, ver P8 em `PENDING.md`. Precisa de:
- Fila de jobs
- Controle de custo por sub-agente
- Isolamento de tools por sub-agente
- Critério de parada
- __Fora do escopo v1__ — mas não mais só brainstorm, é algo que o usuário quer priorizar eventualmente

### App "Copilot" — IA leve rodando no SO

Confirmado pelo usuário (2026-08-02): quer um app leve, em segundo plano, no
espírito do Windows Copilot / Spotlight / PowerToys Run, instalável em
Linux, Windows e Mac. Invocado por atalho global, abre um campo de busca que:

- Busca apps/arquivos/pastas no sistema ("procura pra mim uma pasta X")
- Dá acesso rápido ao Warden sem precisar abrir o app principal
- Totalmente opt-in e configurável — só ativa quem quiser usar assim

Provável relação com a Fase 6 (App Desktop Nativo) — mas é um *modo* diferente
do app principal (overlay leve de busca vs janela de chat completa), então
pode merecer fase própria em vez de virar uma feature a mais da Fase 6.
Ver P9 em `PENDING.md`.

### Dashboard de custo & gerenciamento de chaves de API

Ligado à pendência P4 (controle de custo/rate limit), agora com requisito
explícito de UI: dashboard mostrando consumo de tokens, custo estimado por
provedor/modelo, e uma tela pra cadastrar/gerenciar as chaves de API usadas
(OpenAI, Gemini, etc.). P4 é a parte de backend (onde/como limitar), P10 é a
parte de produto (o que o usuário vê e configura). Ver P10.

### Tela de gerenciamento de integrações MCP

Três frentes, não uma só:

- **Warden como client MCP (integrações prontas)** — conectar em servers MCP
  externos que já existem (Google, GitHub, etc., já previstos na Fase 5.2/5.7),
  com UI pra adicionar/remover. Exemplo concreto do usuário: conectar com o
  **Anchor** (ex-Practice Valuation) via MCP e pedir pro agente criar um
  valuation lá — o Warden não precisa saber nada de Anchor hardcoded, só
  conversa com o MCP server que o Anchor expõe
- **Warden como server MCP** — expor o próprio Warden (tools + vault) como um
  MCP server, pra qualquer app — inclusive de terceiros — poder integrar com
  ele. "Só integra a IA com o que ela quiser" foi como o usuário descreveu
- **Conectores genéricos (P15)** — a parte mais ambiciosa: não ficar restrito
  a integrações que já têm um MCP server pronto no mercado. O usuário quer que
  qualquer pessoa consiga conectar o software dela — mesmo sem MCP pronto —
  e o agente crie/leia algo nesse software. Mecanismo ainda em aberto
  (gerador a partir de spec OpenAPI? assistente guiado que a própria IA usa
  pra "aprender" a integração?)

Ver P11 e P15 em `PENDING.md`.

### "Warden API" — chave de API própria, auto-hospedada, opcional

Ideia central: uma chave de API do **Warden**, não do provedor de IA por trás.
Quem integra não escolhe "GPT" ou "Gemini" — escolhe o Warden, que resolve
sozinho qual modelo usar e já vem com o contexto do vault do usuário embutido.
É a mesma abstração `ModelProvider` que já existe internamente
(`crates/warden-core/src/model`), só que exposta pra fora como produto.

Clarificado pelo usuário (2026-08-02): a chave é criada **dentro do próprio
app** que já está rodando no device do usuário — não é um serviço à parte que
precisa ser contratado ou hospedado em outro lugar. É **totalmente opcional**:
quem não quiser usar isso, não usa, e o resto do Warden funciona igual. Isso
conecta direto com o princípio de "servidor é opcional" (ver `ARCHITECTURE.md`
e P14) — só entra servidor de verdade se a chave precisar ser usada de fora
do device onde o Warden está rodando.

Perguntas de auth, billing e rate limit ainda não foram pensadas. Ver P12.

### Skills configuráveis via UX

Nova ideia do usuário (2026-08-02): além de `Tool` (capacidade em código),
quer um conceito de **Skill** — pacote de instrução/comportamento reutilizável
que o usuário consegue criar **pela interface**, sem escrever código. No
espírito das Skills do próprio Claude. Ainda não definido:

- Skill é só um prompt/instrução empacotada, ou pode compor tools?
- Onde mora (vault? config separada?) e como é versionada
- Como se relaciona com sub-agentes (P8) — uma skill pode ser "invocar um
  sub-agente com esse contexto pronto"?

Ver P16.

### Ecossistema descentralizado (Practice Valuation/Anchor + TruthID)

O Warden não é um projeto isolado — faz parte de um ecossistema open-source
descentralizado que o usuário está construindo, junto com:

- **TruthID** — identidade/autenticação (já citado na Fase 10 como dependência)
- **Practice Valuation** (em processo de rebrand pra **Anchor**) — outro
  produto do usuário
- **Warden** — este projeto

Visão de longo prazo: os três conversam entre si via MCP — o Warden como hub
que integra com os outros produtos do próprio usuário, usando a mesma tela de
integrações MCP (P11) que serve pra integrações de terceiros. Depende desses
outros projetos terem um lado MCP pronto pra integrar. Ver P13.

### Memória vetorial (RAG)

Substituir busca por grep por embedding +相似度 search:
- Indexar vault markdown em banco vetorial (SQLite + extensão, Qdrant, etc.)
- Busca semântica em vez de regex
- Pode conviver com a busca por grep (fallback)

### Plugin system

Permitir que terceiros escrevam plugins sem modificar o core:
- WASM plugins
- MCP servers como padrão de plugins
- Marketplace de plugins

### Voz

- ✅ Entrada por voz (Speech-to-Text) — feito (P28, Sessão 41), via Whisper da OpenAI
- ✅ Resposta por voz (Text-to-Speech) — feito (P28, Sessão 42), via `tts-1` da OpenAI
- Chamada de voz via Telegram/WhatsApp — não iniciado
- **Voz plugável além da OpenAI** (levantado pelo usuário 2026-09-04): hoje STT/TTS estão fixos
  na OpenAI (`transcribe.rs`/`speech.rs`), independente de qual provider de chat está ativo —
  diferente do registry de providers de chat, que já é trocável. Alternativas discutidas, sem
  decisão de prioridade: **Gemini nativo** (já aceita áudio como entrada e tem TTS próprio —
  reaproveitaria a chave do Gemini já cadastrada, sem precisar de conta OpenAI só pra voz);
  **local via `whisper.cpp`** (STT sem chave/custo, mais privado — só cobre entrada, não existe
  TTS local tão simples); **provedor dedicado de terceiros** (ex. ElevenLabs, quando qualidade de
  voz sintetizada importa mais). Usuário optou por não implementar agora (2026-09-04) — registrar
  como ideia de backlog. Se algum dia for retomado, o trabalho maior é abstrair uma trait de voz
  de verdade (tipo `SpeechProvider`, plugável como `ModelProvider` já é) em vez de só trocar o
  endpoint fixo

### Memória compartilhada entre múltiplos agentes

Vários Wardens (um por contexto) compartilhando um vault comum:
- Agente pessoal
- Agente de trabalho
- Agente de estudos
- Cada um com seu contexto, mas todos acessando a mesma base

### Integração com Home Assistant

Warden como interface de IA para casa inteligente.

---

## Backlog

| Item | Notas |
|---|---|
| Streaming de respostas (SSE no canal HTTP) | UX melhor que esperar resposta completa |
| Histórico de conversas pesquisável | Indexar conversas no vault |
| Múltiplos perfis de agente | Um agente "formal" e um "casual" |
| Exportação de memória | ZIP com todo o vault markdown |
| Comandos de voz (skill) | "Warden, lembre-me de..." |