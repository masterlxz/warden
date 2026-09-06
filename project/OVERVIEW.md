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
Fase 2 — Canal Telegram              [x] Concluída
Fase 3 — Canal WhatsApp              [x] Concluída
Fase 4 — Vault & Memória             [ ] Pendente (repensar com Arweave — ver PENDING.md P24)
Fase 5 — Tools & MCP                 [~] Quase completa (falta só 5.4, bloqueada pela Fase 8)
Fase 6 — App Desktop Nativo          [x] Concluída (polish: registry de provedores ✓ Sessão 35)
Fase 7 — App Mobile                  [ ] Pendente (7.1-7.4 concluídas — chat real + arquivos do celular; falta 7.5-7.6)
Fase 8 — Extensão de Navegador       [ ] Pendente
Fase 9 — Rede de Nós & Tailscale     [ ] Pendente
Fase 10 — Autenticação & TruthID     [ ] Pendente
```