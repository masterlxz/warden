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
| P8 | **Canal Terminal cross-platform + lançamento de agentes** — como estruturar um canal terminal completo (tipo Claude Code, não focado em programação) rodando em Linux/Windows/Mac; qual crate de TUI (se houver, ex. `ratatui`); como a tool `shell` (Fase 5.5) fica disponível a partir de qualquer canal, não só do terminal; e como "lançar agentes" se relaciona com sub-agentes leves (1.8) vs sub-agentes autônomos (fora de escopo v1) | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P9 | **App "Copilot" no SO** — launcher leve tipo Spotlight/PowerToys Run, cross-platform (Linux/Windows/Mac), roda em segundo plano, atalho global abre campo de busca pra apps/arquivos/pastas do sistema + acesso rápido ao Warden. Opt-in/configurável. Relação com a Fase 6 (App Desktop) ainda não definida — modo do mesmo app ou fase própria | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P10 | **Dashboard de custo & gerenciamento de chaves de API** — UI de consumo de tokens, custo estimado por provedor/modelo, cadastro/gerenciamento de chaves. Relacionado a P4 (controle de custo/rate limit), que é a parte de backend disso | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P11 | **Tela de gerenciamento de integrações MCP** — duas direções distintas: (a) Warden como *client* MCP, conectando em servers externos (já previsto na Fase 5.2/5.7) e (b) Warden como *server* MCP, expondo suas próprias tools/vault pra qualquer app de terceiro integrar | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P12 | **"Warden API"** — chave de API do próprio Warden (não do provedor por trás): quem integra escolhe o Warden, não o modelo — a abstração `ModelProvider` que hoje é interna vira produto exposto, já vem com o contexto do vault do usuário embutido. Levanta decisões de auth, billing e rate limit ainda não pensadas | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P13 | **Integração via MCP com o ecossistema do usuário** — Practice Valuation (rebrand pra Anchor) e TruthID. Depende desses projetos terem um lado MCP pronto — bloqueado por fora do Warden | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |

### Funcionalidades Pendentes

| ID | Item | Prioridade |
|---|---|---|
| P7 | **Todas as fases 1-10** — projeto recém-criado, nenhuma fase iniciada | — |

---

## Resolvidas

| ID | Item | Resolução | Sessão |
|---|---|---|---|
| P2 | **Framework de CLI** — clap vs structopt vs gum | **clap v4 (derive)** — structopt descontinuado/incorporado ao clap; gum não se aplica a Rust | Sessão 2 (2026-08-01) |