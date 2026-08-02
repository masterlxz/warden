# Fases Detalhadas — Planejamento Global

> **Nota**: Este é o planejamento inicial do projeto, criado a partir do documento de visão original.
> As fases serão detalhadas e ajustadas conforme o progresso.

---

### Fase 1 — Fundação & Orquestrador

**Objetivo**: Ter um agente funcional via terminal/CLI — orquestrador falando com 1 modelo,
lendo/escrevendo num vault markdown local, sem canal externo ainda.

**Stack**: Rust (core), TypeScript (CLI se necessário)

**Etapas**:
- [x] 1.1 — Setup do projeto Rust + estrutura de módulos (orchestrator, model, memory, tool)
- [x] 1.2 — Trait `ModelProvider` + implementação OpenAI (primeiro provedor)
- [x] 1.3 — Loop de conversa via stdin/stdout (CLI)
- [x] 1.4 — Vault markdown local: ler/escrever arquivos `.md` em pasta configurável
- [x] 1.5 — Memória: buscar contexto relevante no vault (grep/ripgrep) e injetar no prompt
- [x] 1.6 — Trait `Tool` + primeira tool: `read_file`, `write_file`
- [x] 1.7 — Tool `web_search` (pesquisa na internet via API)
- [ ] 1.8 — Sub-agente leve: delegar tarefa escopada pra outro modelo/contexto
- [ ] 1.9 — Testes de integração do pipeline completo (CLI)
- [ ] 1.10— Configuração via arquivo YAML/TOML (modelo, API keys, vault path)

**Decisões pendentes**:
- Formato do prompt de sistema (persona configurável)

---

### Fase 2 — Canal Telegram

**Objetivo**: Primeiro canal externo funcionando — mais simples e sem risco de ban.

**Stack**: Rust (reqwest + Bot API) ou TypeScript

**Etapas**:
- [ ] 2.1 — Setup do bot Telegram (token, webhook/polling)
- [ ] 2.2 — Implementar trait `Channel` para Telegram
- [ ] 2.3 — Receber mensagens e rotear para o orquestrador
- [ ] 2.4 — Enviar respostas de volta
- [ ] 2.5 — Suporte a markdown/markdownV2 nas mensagens
- [ ] 2.6 — Comandos básicos: /start, /help
- [ ] 2.7 — Gerenciamento de conversas (thread por chat)
- [ ] 2.8 — Testes de integração

---

### Fase 3 — Canal WhatsApp

**Objetivo**: Segundo canal externo, usando Baileys como sidecar Node.

**Stack**: Node.js (Baileys), IPC com core Rust via socket local

**Etapas**:
- [ ] 3.1 — Setup do sidecar Node.js com Baileys
- [ ] 3.2 — Autenticação via QR code (whatsapp-web.js style)
- [ ] 3.3 — IPC entre sidecar e core Rust (stdin/stdout ou socket)
- [ ] 3.4 — Receber mensagens e rotear para o orquestrador
- [ ] 3.5 — Enviar respostas de volta
- [ ] 3.6 — Gerenciamento de sessão (reconnect, keepalive)
- [ ] 3.7 — Tratamento de mídia (imagem, áudio, documento)
- [ ] 3.8 — Testes de integração

---

### Fase 4 — Vault & Memória

**Objetivo**: Memória persistente com backup descentralizado.

**Stack**: Rust, IPFS (Filebase/Pinata)

**Etapas**:
- [ ] 4.1 — Espelhar vault local em IPFS (pin via Filebase + Pinata)
- [ ] 4.2 — Cifra opcional do vault (AES-256-GCM, mesmo padrão TruthID Vault)
- [ ] 4.3 — Versionamento de memória (histórico de mudanças)
- [ ] 4.4 — Busca semântica no vault (embedding local ou via API)
- [ ] 4.5 — Backup automático em intervalo configurável
- [ ] 4.6 — Restore a partir de snapshot IPFS
- [ ] 4.7 — Configuração de providers de pinning

---

### Fase 5 — Tools & MCP

**Objetivo**: Sistema de ferramentas extensível, compatível com MCP.

**Stack**: Rust (core), qualquer linguagem para MCP servers

**Etapas**:
- [ ] 5.1 — Registry de tools (`ToolProvider` trait)
- [ ] 5.2 — MCP client: conectar em MCP servers externos
- [ ] 5.3 — Tool `web_search` via MCP
- [ ] 5.4 — Tool `browser` (via extensão — ver Fase 8)
- [ ] 5.5 — Tool `shell` (executar comando no nó cliente)
- [ ] 5.6 — Tool `file_system` (ler/escrever arquivos no nó cliente)
- [ ] 5.7 — Integração Google (Gmail, Drive, Calendar) via MCP servers existentes
- [ ] 5.8 — Rate limiting e controle de custo por tool

---

### Fase 6 — App Desktop Nativo

**Objetivo**: Aplicação desktop nativa com Tauri — mesma stack do TruthID.

**Stack**: Tauri + Rust + React + TypeScript

**Etapas**:
- [ ] 6.1 — Setup Tauri + React + TypeScript
- [ ] 6.2 — Shell do app: sidebar de conversas, área de chat
- [ ] 6.3 — Integração com o core (IPC Rust↔frontend)
- [ ] 6.4 — Canal nativo (chat direto no app)
- [ ] 6.5 — Configuração visual (modelo, API keys, canais)
- [ ] 6.6 — Histórico de conversas
- [ ] 6.7 — Renderização de markdown nas mensagens
- [ ] 6.8 — Build Linux/Windows/macOS

---

### Fase 7 — App Mobile

**Objetivo**: Warden no celular como cliente (nunca servidor).

**Stack**: Tauri Mobile (Rust + React)

**Etapas**:
- [ ] 7.1 — Setup Tauri Mobile (Android + iOS)
- [ ] 7.2 — Conectar ao servidor (Tailscale + WebSocket/gRPC)
- [ ] 7.3 — Interface de chat mobile
- [ ] 7.4 — Execução de tools local (shell, arquivos)
- [ ] 7.5 — Notificações push
- [ ] 7.6 — Build e deploy

---

### Fase 8 — Extensão de Navegador

**Objetivo**: Extensão Chrome/Firefox que funciona como canal de chat + tool provider de browser.

**Stack**: Web Extension (Manifest V3), TypeScript

**Etapas**:
- [ ] 8.1 — Setup da extensão (Manifest V3, popup, background script)
- [ ] 8.2 — Canal de chat (popup com conversa)
- [ ] 8.3 — Tool provider: ler DOM da página ativa
- [ ] 8.4 — Tool provider: clicar em elementos
- [ ] 8.5 — Tool provider: navegar para URL
- [ ] 8.6 — Tool provider: extrair texto/seleção
- [ ] 8.7 — Comunicação com o servidor Warden (WebSocket)
- [ ] 8.8 — Publicação na Chrome Web Store / Firefox Add-ons

---

### Fase 9 — Rede de Nós & Tailscale

**Objetivo**: Múltiplos clientes conectados ao servidor, execução remota de tools.

**Stack**: Tailscale, WebSocket/gRPC, Rust

**Etapas**:
- [ ] 9.1 — Setup Tailscale (todos os nós na mesma subnet)
- [ ] 9.2 — Protocolo servidor↔cliente (WebSocket ou gRPC)
- [ ] 9.3 — Registrar cliente no servidor (pareamento)
- [ ] 9.4 — Rotear requisição de tool para o cliente correto
- [ ] 9.5 — Cliente executa tool localmente e devolve resultado
- [ ] 9.6 — Workspace de máquinas (ver/gerenciar nós conectados)
- [ ] 9.7 — Pareamento de cliente novo via QR code (mesmo padrão TruthID)

---

### Fase 10 — Autenticação & TruthID

**Objetivo**: Integrar login TruthID para autenticação e workspace de máquinas.

**Stack**: TruthID SDK, Rust, TypeScript

**Etapas**:
- [ ] 10.1 — Login via TruthID no app desktop
- [ ] 10.2 — Workspace: listar dispositivos pareados via Device Registry do TruthID
- [ ] 10.3 — Substituir auth local por TruthID (quando TruthID estiver em release estável)
- [ ] 10.4 — Deep link pareamento mobile↔desktop