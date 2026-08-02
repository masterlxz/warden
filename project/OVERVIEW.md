# O que é o Warden

Agente de IA pessoal model-agnostic, memória em markdown com backup em IPFS,
multi-canal (WhatsApp, Telegram, app desktop, app mobile, extensão de navegador),
rodando como uma rede de nós com um nó servidor central.

Stack planejada:
- **Core/Runtime**: Rust
- **Desktop**: Tauri + Rust + React + TypeScript
- **Mobile**: Tauri (mesmo codebase, build mobile)
- **Extensão**: Web Extension (Manifest V3)
- **Memória**: Markdown vault (Obsidian-compatível) espelhado em IPFS
- **Rede**: Tailscale (malha entre nós)
- **Canais**: Baileys (Node.js sidecar) para WhatsApp, Bot API para Telegram
- **Orquestrador**: Model-agnostic (suporta OpenAI, Anthropic, Gemini, etc.)

---

# Status Geral

```
Fase 1 — Fundação & Orquestrador    [x] Concluída
Fase 2 — Canal Telegram              [ ] Pendente
Fase 3 — Canal WhatsApp              [ ] Pendente
Fase 4 — Vault & Memória             [ ] Pendente
Fase 5 — Tools & MCP                 [ ] Pendente
Fase 6 — App Desktop Nativo          [ ] Pendente
Fase 7 — App Mobile                  [ ] Pendente
Fase 8 — Extensão de Navegador       [ ] Pendente
Fase 9 — Rede de Nós & Tailscale     [ ] Pendente
Fase 10 — Autenticação & TruthID     [ ] Pendente
```