# Roadmap e Evoluções Planejadas

## Sequenciamento Sugerido

> **Atualizado em 2026-08-02** — decisão do usuário: priorizar o App Desktop (Fase 6) logo
> depois da Fase 1, antes dos canais de mensageria (Telegram/WhatsApp) e do resto da Fase
> 4/5. Motivo: interface de chat de verdade importa mais agora do que canais externos, e
> tecnicamente não há bloqueio — o app desktop só faz IPC local Rust↔frontend (não depende da
> topologia servidor↔cliente, que só entra na Fase 9; ver P14 em `PENDING.md`). Os números das
> fases em `PHASE.md` não mudaram — só a ordem de execução abaixo.

Ordem de implementação recomendada:

1. **Orquestrador CLI + 1 modelo + vault local** (Fase 1) — base de tudo ✅ concluída
2. **App Desktop** (Fase 6) — interface nativa de chat, valor mais visível pro usuário agora
3. **Canal Telegram** (Fase 2) — primeiro canal externo, mais simples
4. **Canal WhatsApp** (Fase 3) — sidecar Node via Baileys
5. **Vault espelhado em IPFS** (Fase 4) — redundância da memória
6. **Tools & MCP** (Fase 5) — extensibilidade (parte já implementada ad-hoc na Fase 1: `read_file`,
   `write_file`, `web_search`, `delegate_task`)
7. **Rede de nós + Tailscale** (Fase 9) — execução remota de tools
8. **Extensão de Navegador** (Fase 8) — canal + tool de browser
9. **App Mobile** (Fase 7) — cliente móvel
10. **Integração TruthID** (Fase 10) — autenticação unificada

---

## Ideias de Expansão (Brainstorm — sem `/plan`)

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

- Entrada por voz (Speech-to-Text)
- Resposta por voz (Text-to-Speech)
- Chamada de voz via Telegram/WhatsApp

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