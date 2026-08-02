# Pendências do Projeto

> Arquivo central de pendências — **resolvidas e não resolvidas**.
> Toda pendência encontrada em qualquer arquivo do projeto deve ser registrada aqui com um ID único.
> Ao resolver uma, marcar como `✅ Resolvida` com a sessão em que foi corrigida.
>
> Última atualização: 2026-07-31 (Criação do projeto)

---

## Não Resolvidas

### Decisões em Aberto

| ID | Item | Onde se originou | Prioridade |
|---|---|---|---|
| P1 | **Protocolo servidor↔cliente** — gRPC vs WebSocket. O TruthID já tem um relay stateless por WS — reaproveitar seria natural. | `ARCHITECTURE.md` | 🔴 Alta |
| P3 | **Formato do prompt de sistema** — como estruturar a persona configurável do agente | `PHASE.md` (Fase 1) | 🟠 Média |
| P4 | **Controle de custo/rate limit** — onde e como limitar chamadas de modelo por usuário/período | Visão original | 🟠 Média |
| P5 | **Estratégia de busca no vault** — grep simples vs fuzzy finder vs embedding (Fase 4) | `PHASE.md` (Fase 1) | 🟡 Baixa |
| P6 | **Estratégia de busca semântica** — embedding local (BERT.cpp) vs API externa (OpenAI) | `PHASE.md` (Fase 4) | 🟡 Baixa |

### Funcionalidades Pendentes

| ID | Item | Prioridade |
|---|---|---|
| P7 | **Todas as fases 1-10** — projeto recém-criado, nenhuma fase iniciada | — |

---

## Resolvidas

| ID | Item | Resolução | Sessão |
|---|---|---|---|
| P2 | **Framework de CLI** — clap vs structopt vs gum | **clap v4 (derive)** — structopt descontinuado/incorporado ao clap; gum não se aplica a Rust | Sessão 2 (2026-08-01) |