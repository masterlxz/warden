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
| P6 | **Estratégia de busca semântica** — embedding local (BERT.cpp) vs API externa (OpenAI), pra Fase 4 (quando o grep simples da v1 não bastar mais) | `PHASE.md` (Fase 4) | 🟡 Baixa |
| P8 | **Canal Terminal cross-platform + lançamento de agentes** — como estruturar um canal terminal completo (tipo Claude Code, não focado em programação) rodando em Linux/Windows/Mac; qual crate de TUI (se houver, ex. `ratatui`); e como "lançar agentes" se relaciona com sub-agentes leves (1.8) vs sub-agentes autônomos (fora de escopo v1). **Atualizado 2026-08-04**: a tool `shell` em si (Fase 5.5) já está implementada e disponível de qualquer canal que passe por `bootstrap()` (CLI, desktop) — o que resta em aberto aqui é só a arquitetura do canal Terminal dedicado (TUI etc.), não mais a tool | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P9 | **App "Copilot" no SO** — launcher leve tipo Spotlight/PowerToys Run, cross-platform (Linux/Windows/Mac), roda em segundo plano, atalho global abre campo de busca pra apps/arquivos/pastas do sistema + acesso rápido ao Warden. Opt-in/configurável. Relação com a Fase 6 (App Desktop) ainda não definida — modo do mesmo app ou fase própria | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P10 | **Dashboard de custo & gerenciamento de chaves de API** — UI de consumo de tokens, custo estimado por provedor/modelo, cadastro/gerenciamento de chaves. Relacionado a P4 (controle de custo/rate limit), que é a parte de backend disso | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P11 | **Tela de gerenciamento de integrações MCP** — duas direções distintas: (a) Warden como *client* MCP, conectando em servers externos já prontos (já previsto na Fase 5.2/5.7) — ex. concreto do usuário: conectar com o Anchor via MCP e mandar o agente criar um valuation lá; e (b) Warden como *server* MCP, expondo suas próprias tools/vault pra qualquer app de terceiro integrar | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟠 Média |
| P12 | **"Warden API"** — chave de API criada facilmente **dentro do próprio app**, hospedada no "app host" do Warden (o app que já está rodando no device do usuário) — não é um serviço à parte. Totalmente autossuficiente e **opcional**: não precisa de servidor central pra existir, a menos que o usuário queira usá-la de fora do próprio device (aí entra em jogo P14). Quem integra escolhe o Warden, não o modelo — a abstração `ModelProvider` já vem com o contexto do vault embutido | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P13 | **Integração via MCP com o ecossistema do usuário** — Practice Valuation (rebrand pra Anchor) e TruthID. Depende desses projetos terem um lado MCP pronto — bloqueado por fora do Warden | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P15 | **Conectores MCP genéricos / fáceis de criar** — não ficar restrito a integrações MCP que já existem prontas no mercado. O usuário quer facilitar que qualquer pessoa conecte o software dela (mesmo sem um MCP server pronto pra ele) com o Warden, e o agente consiga criar/ler algo nesse software. Precisa definir o mecanismo — gerador de conector a partir de API/OpenAPI spec? Template guiado por UX? | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |
| P16 | **Sistema de "Skills"** — usuário quer um conceito de skills no Warden, criáveis pela UX (não só por código), no espírito das Skills do Claude — pacotes de instrução/capacidade reutilizáveis. Arquitetura ainda não definida: como isso difere de `Tool` (código) e como interage com Sub-agentes (P8) | `ROADMAP.md`, pedido do usuário 2026-08-02 | 🟡 Baixa |

### Funcionalidades Pendentes

| ID | Item | Prioridade |
|---|---|---|
| P7 | **Todas as fases 1-10** — projeto recém-criado, nenhuma fase iniciada | — |

---

## Resolvidas

| ID | Item | Resolução | Sessão |
|---|---|---|---|
| P2 | **Framework de CLI** — clap vs structopt vs gum | **clap v4 (derive)** — structopt descontinuado/incorporado ao clap; gum não se aplica a Rust | Sessão 2 (2026-08-01) |
| P14 | **O que realmente exige servidor** | **Mapeado** em `ARCHITECTURE.md` ("Mapa de dependência de servidor") — só a Fase 9 (rede de nós/execução remota) exige de fato o papel servidor; canais Telegram/WhatsApp precisam só de "algo sempre ligado" (não é topologia servidor↔cliente); Warden API (P12) e Warden como server MCP (P11b) só precisam de servidor se chamados de fora do device. Pode ser revisado quando a Fase 9 for implementada de verdade | Sessão 7 (2026-08-02) |
| P5 | **Estratégia de busca no vault** — grep simples vs fuzzy finder vs embedding (Fase 4) | **Grep simples (substring, case-insensitive, por palavra)** para v1 — já implementado em `Vault::search`. Fuzzy/embedding fica pra Fase 4 (ver P6) se o grep simples não bastar | Sessão 8 (2026-08-02) |