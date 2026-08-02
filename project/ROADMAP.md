# Roadmap e Evoluções Planejadas

## Sequenciamento Sugerido

Ordem de implementação recomendada, baseada em dependências técnicas:

1. **Orquestrador CLI + 1 modelo + vault local** (Fase 1) — base de tudo
2. **Canal Telegram** (Fase 2) — primeiro canal externo, mais simples
3. **Canal WhatsApp** (Fase 3) — sidecar Node via Baileys
4. **Vault espelhado em IPFS** (Fase 4) — redundância da memória
5. **Tools & MCP** (Fase 5) — extensibilidade
6. **App Desktop** (Fase 6) — interface nativa
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