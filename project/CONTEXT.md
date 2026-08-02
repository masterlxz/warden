# Warden — PRD v1.0

## Vision

Warden é um agente de IA pessoal que o usuário realmente controla — model-agnostic,
com memória própria em markdown, backup descentralizado em IPFS, e acesso de qualquer
lugar (WhatsApp, Telegram, app nativo, navegador).

O sistema prioriza:

- Privacidade dos dados do usuário
- Model-agnostic (trocar de modelo sem perder memória)
- Código aberto
- Autonomia (sem dependência de um serviço cloud central)
- Extensibilidade via tools/MCP

---

## Core Problem

Assistentes de IA hoje são:

- Presos a um único provedor (ChatGPT, Claude, Gemini)
- Sem memória persistente portátil
- Dados presos no ecossistema do fornecedor
- Sem acesso a ferramentas externas (navegador, arquivos, shell)

Warden resolve isso sendo:

- Model-agnostic — escolha o modelo que quiser
- Memória em markdown — portátil, legível por humanos, versionável
- Backup em IPFS — seus dados não morrem se o computador quebrar
- Multi-canal — use de qualquer lugar
- Tools como cidadãos de primeira classe — shell, arquivos, navegador

---

## Core Concepts

### Agente

O agente é uma persona configurável (`agent_name`). Ele tem:

- Uma memória (vault markdown)
- Acesso a tools (MCP-style)
- Um modelo de IA configurável
- Múltiplos canais de entrada/saída

### Servidor

Nó central que roda o orquestrador:

- Escolha de modelo
- Gestão de memória
- Decisão de quais tools chamar
- Roteamento de mensagens entre canais

### Cliente

Nós periféricos que se conectam ao servidor:

- Mandam/recebem mensagens
- Executam tools localmente quando o servidor pede
- Tipos: desktop, mobile, extensão de navegador

### Memória (Vault)

Armazenamento em markdown:

- Formato legível por humanos (Obsidian-compatível)
- Espelhado em IPFS para redundância
- Cifrado (quando aplicável)
- Versionado

### Canal

Interface de comunicação com o usuário:

- WhatsApp (via Baileys, Node.js sidecar)
- Telegram (Bot API, HTTP nativo)
- App nativo (desktop/mobile Tauri)
- Extensão de navegador
- Terminal (CLI interativo, estilo Claude Code — não é assistente de programação,
  foco em produtividade geral de linha de comando; cross-platform Linux/Windows/Mac)

---

## User Flow

### Setup Inicial

1. Usuário instala o Warden no servidor (homelab/desktop)
2. Configura modelo de IA (API key)
3. Configura canais (Telegram token, WhatsApp)
4. Warden cria vault de memória local
5. Usuário começa a conversar

### Conversa Multi-canal

1. Usuário manda mensagem pelo Telegram
2. Servidor recebe, processa no orquestrador
3. Orquestrador decide se precisa de tool (ex: pesquisar na web)
4. Executa tool, obtém resultado
5. Gera resposta, envia de volta pelo Telegram
6. Registra na memória

### Execução Remota de Tool

1. Servidor decide que precisa executar algo no computador do usuário
2. Envia requisição para o nó cliente apropriado
3. Cliente executa localmente (shell, arquivo, browser)
4. Cliente devolve resultado
5. Servidor continua o fluxo

---

## Security Requirements

- Comunicação servidor↔cliente criptografada (Tailscale)
- Chaves de API armazenadas localmente
- Canais com autenticação (Token do Telegram, QR pareamento WhatsApp)
- Memória em IPFS pode ser cifrada
- Extensão de navegador com permissões escopadas

---

## Non Goals

- Sub-agentes autônomos criando outros agentes recursivamente (fase futura)
- Controle de tela (remote desktop) — usar extensão de navegador em vez disso
- Login via TruthID no v1 (pode vir depois)
- Integração Google nativa (via MCP servers existentes)