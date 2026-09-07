# O que é o Warden

Agente de IA pessoal model-agnostic, memória em markdown com sync descentralizado via Arweave,
multi-canal (WhatsApp, Telegram, app desktop, app mobile, extensão de navegador),
rodando como uma rede de nós com um nó servidor central.

Stack planejada:
- **Core/Runtime**: Rust
- **Desktop**: Tauri + Rust + React + TypeScript
- **Mobile**: Tauri (mesmo codebase, build mobile)
- **Extensão**: Web Extension (Manifest V3)
- **Memória**: Markdown vault (Obsidian-compatível), sync manual via Arweave (paga pelo TruthID)
- **Rede**: Tailscale (malha entre nós)
- **Canais**: Baileys (Node.js sidecar) para WhatsApp, Bot API para Telegram
- **Orquestrador**: Model-agnostic (suporta OpenAI, Anthropic, Gemini, etc.)

---

# Status Geral

```
Fase 1 — Fundação & Orquestrador    [x] Concluída
Fase 2 — Canal Telegram              [x] Concluída
Fase 3 — Canal WhatsApp              [x] Concluída
Fase 4 — Vault & Memória             [~] Quase completa (4.1-4.3 concluídas — sync via Arweave
                                          no desktop/CLI; falta 4.4 mobile, 4.5 busca semântica)
Fase 5 — Tools & MCP                 [~] Quase completa (falta só 5.4, bloqueada pela Fase 8)
Fase 6 — App Desktop Nativo          [x] Concluída (polish: registry de provedores ✓ Sessão 35)
Fase 7 — App Mobile                  [ ] Pendente (7.1-7.4 concluídas — chat real + arquivos do celular; falta 7.5-7.6)
Fase 8 — Extensão de Navegador       [ ] Pendente
Fase 9 — Rede de Nós & Tailscale     [ ] Pendente
Fase 10 — Autenticação & TruthID     [ ] Pendente
```