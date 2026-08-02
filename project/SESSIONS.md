# Log de Sessões

> **Nota**: Este log foi criado junto com o projeto. As sessões serão registradas aqui conforme o trabalho avança.
>
> Última atualização: 2026-08-02 (Sessão 11)

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