# Decisões de Arquitetura

## Registro de Decisões

| Decisão | Opções | Status |
|---|---|---|
| Framework desktop/mobile | Tauri vs Electron vs nativo | **Tauri** ✓ — reaproveita stack Rust/TS já usada no TruthID |
| Topologia de rede | Estrela vs Malha P2P | **Estrela** ✓ — servidor central, clientes se conectam. **Refinado 2026-08-02**: servidor é opcional, só entra quando a feature exige coordenação entre múltiplos nodes — ver nota abaixo e P14 em `PENDING.md` |
| Memória | Markdown vault (Obsidian) vs banco vetorial | **Markdown vault** ✓ — portátil, legível, versionável |
| Backup | IPFS (Filebase/Pinata) vs S3 vs auto | **IPFS** ✓ — mesmo padrão do TruthID Vault |
| Model-agnostic | Camada de abstração vs hardcoded | **Camada de abstração** ✓ — suporta OpenAI, Anthropic, Gemini |
| WhatsApp | Baileys (Node.js) vs nativo Rust | **Baileys (sidecar Node)** ✓ — não vale reescrever em Rust |
| Telegram | Bot API HTTP vs MTProto | **Bot API HTTP** ✓ — mais simples, sem risco de ban |
| Protocolo servidor↔cliente | gRPC vs WebSocket vs HTTP | **Em aberto** — candidatos: WebSocket (reaproveitar relay do TruthID) ou gRPC |
| Autenticação servidor↔cliente | Chave local vs TruthID | **Chave local no v1** ✓ — TruthID pode vir depois |
| Extensão de navegador | Canal + Tool provider vs só canal | **Canal + Tool provider** ✓ — expõe DOM/clique/navegação como tool MCP |
| Framework de CLI | clap vs structopt vs gum | **clap v4 (derive)** ✓ — structopt foi descontinuado e incorporado ao clap desde a v3; gum é ferramenta Bash/TUI, não se aplica a Rust |
| API de `web_search` | Tavily vs Brave Search API vs Google Custom Search | **Tavily** ✓ — feita pra tool use de agentes LLM (resultados já vêm resumidos), free tier de 1.000 buscas/mês sem cartão de crédito. Tool só é registrada se `TAVILY_API_KEY` estiver setada (degradação graciosa, mesmo espírito de P14) |
| Formato do arquivo de config | TOML vs YAML | **TOML** ✓ — convenção do próprio ecossistema Rust (mesmo formato do `Cargo.toml`), sem as ambiguidades clássicas de parsing do YAML, crate `toml` madura e serde-native. Localização default via crate `dirs` (`~/.config/warden/config.toml` no Linux, equivalente no Windows/macOS), com override via `--config`. Precedência: flag de CLI > variável de ambiente (só pra API keys) > arquivo de config > default embutido |
| Lógica de bootstrap compartilhada entre canais | Duplicar em cada canal vs crate à parte vs meter em `warden-core` | **Novo crate `crates/warden-bootstrap`** ✓ — carrega o config TOML e monta o `Orchestrator` (provider/vault/tools/sub-agente) uma vez só, usado por `warden-cli` e por `desktop/src-tauri`. Não entrou em `warden-core` de propósito (fonte de config é decisão de camada de aplicação/canal, não do motor model-agnostic); não virou "warden-cli como lib" pra não misturar parsing de CLI (clap) com algo que um app GUI também precisa. `default_vault_path` é parâmetro da função `bootstrap(...)` — cada canal informa o fallback certo pro seu contexto (CLI: `"vault"` relativo ao cwd; desktop: `~/Warden/vault`, já que o cwd de um app lançado por ícone é imprevisível) |
| Formato/local do histórico de conversas (6.6) | JSON no config dir vs dentro do vault markdown vs banco embutido (sqlite) | **JSON, um arquivo por conversa, em `~/.config/warden/conversations/<id>.json`** ✓ — mesmo `dirs::config_dir()` já usado pro `config.toml`, por ser dado opaco de app (não é conhecimento humano-navegável tipo o vault, que fica de propósito fora dessa pasta). Um arquivo por conversa (não um índice único) evita reescrever tudo a cada mensagem e isola corrupção — `list_conversations` já pula arquivo malformado em vez de falhar a lista inteira. Lógica em `warden-bootstrap` (`Conversation`/`ConversationMessage`/`save_conversation`/`list_conversations`), mesmo padrão de `save_config`/`load_config_from_path`; comandos Tauri (`list_conversations`/`save_conversation`) só chamam essas funções |
| Build/release do app desktop (6.8) | GitHub Actions (matrix por OS) vs cross-compilação local vs serviço de build de terceiro | **GitHub Actions, `.github/workflows/build.yml`** ✓ — replica quase literalmente o `build.yml` do TruthID (mesmo stack Tauri v2): dispara em tag `v*`, matrix `ubuntu-22.04`/`windows-latest`/`macos-latest`, `tauri-apps/tauri-action@v0` builda e publica um GitHub Release **draft** por tag. Sem assinatura de código nem notarização macOS (mesma decisão já tomada no TruthID — não bloqueante pro v1). Cross-compilação local nunca foi opção real: não tem SDK da Apple nem toolchain Windows nesta máquina de dev (Arch Linux) |
| Segurança da tool `shell` (5.5) | Sempre ativa (mesmo padrão de `read_file`/`write_file`) vs opt-in vs allowlist/sandboxing | **Opt-in, desligada por padrão** ✓ — decisão explícita do usuário, não da IA: uma tool de shell é categoricamente mais arriscada que as tools de arquivo (execução arbitrária de comando vs leitura/escrita já sem scoping real). Gate via `resolve_flag(WARDEN_ENABLE_SHELL, config.enable_shell)` (mesma precedência env > config do `resolve_secret`, mas pra booleano), com toggle também na tela de Settings do desktop (`enable_shell` em `SettingsSnapshot`/`SettingsFormPayload`) — religar/desligar já dá live-reload do orchestrator de graça, reaproveitando o mecanismo da 6.5. **Nenhum sandboxing/allowlist além disso** — uma vez ligada, o modelo pode rodar qualquer comando; as únicas proteções são `timeout_ms` (default 30s, capado em 5min, mata o processo via `kill_on_drop`) e truncamento de stdout/stderr (~20KB) pra não estourar o contexto. Execução via `sh -c` (Unix) / `cmd /C` (Windows) — PowerShell fica de fora por ora. `cwd` default = raiz do vault, overridável (relativo ou absoluto) |

---

## Topologia: Estrela (Servidor + Clientes)

```
┌─────────────┐
│  Servidor   │ (homelab/desktop fixo)
│  - Modelo   │
│  - Memória  │
│  - Tools    │
└──────┬──────┘
       │ Tailscale
       ├──────────────────┐
┌──────┴──────┐    ┌──────┴──────┐
│  Cliente    │    │  Cliente    │
│  Desktop    │    │  Mobile     │
└─────────────┘    └─────────────┘

┌─────────────┐    ┌─────────────┐
│  Extensão   │    │  Telegram   │
│  Browser    │    │  WhatsApp   │
└─────────────┘    └─────────────┘
```

### Servidor é opcional (refinamento 2026-08-02)

O diagrama acima é o caso de **múltiplos nodes**. Mas um único node (ex: só o
app rodando no celular do usuário, sem nenhum outro dispositivo) deve
funcionar **100% standalone, sem nenhum servidor central** — cliente e
servidor colapsam no mesmo processo local.

Princípio: o servidor só existe pra resolver o que **de fato** exige
coordenação entre múltiplos nodes. Fora isso, não é necessário.

Reframe importante: "servidor" aqui **não é uma categoria de infraestrutura
à parte**. É sempre um device que o próprio usuário já tem (o desktop fixo, o
homelab) só que designado como "o que fica sempre ligado". Não existe nenhum
cenário no Warden que exige infraestrutura de terceiro que o usuário não
controle — o pior caso é "preciso que uma das minhas próprias máquinas
fique ligada", nunca "preciso alugar/operar um serviço".

### Mapa de dependência de servidor (P14)

| Precisa de servidor? | Feature |
|---|---|
| ❌ Não | Orquestrador, model calls, tools, vault local (Fase 1) |
| ❌ Não | Canal Terminal (Fase 1.3 / canal Terminal completo, P8) |
| ❌ Não | App Desktop ou Mobile rodando sozinho, 1 device (Fases 6/7) |
| ❌ Não | Warden como *client* MCP — conectar em servers externos, ex. Anchor (P11a) |
| ❌ Não | Sub-agente leve — invocação síncrona, mesmo processo (1.8) |
| ❌ Não | Backup em IPFS — usa Filebase/Pinata, não infra própria (Fase 4) |
| ❌ Não | Warden API (P12) ou Warden como *server* MCP (P11b), **se só chamado do mesmo device** (localhost) |
| ⚠️ Precisa de "algo sempre ligado" — não é topologia servidor↔cliente, é só uptime | Canal Telegram (long polling, Fase 2) |
| ⚠️ Precisa de "algo sempre ligado" | Canal WhatsApp (sessão Baileys tem que ficar conectada, Fase 3) |
| ⚠️ Precisa ser alcançável de fora do device | Warden API (P12) chamada de fora do device onde o Warden roda |
| ⚠️ Precisa ser alcançável de fora do device | Warden como *server* MCP (P11b) pra app remoto/cloud, não local |
| ✅ Sim — precisa do node primário (papel "servidor" de fato) | Execução remota de tool em outro device (Fase 9) |
| ✅ Sim | Pareamento / workspace de múltiplos devices (Fase 9.6/9.7) |
| ✅ Sim | Extensão de navegador falando com orquestrador rodando em **outro** device (se for o mesmo device, cai no ❌) |

Conclusão: só a Fase 9 (rede de nós) exige de fato o papel "servidor" da
topologia estrela. Tudo mais é local por padrão, ou vira servidor só na
medida em que o usuário decide expor pra fora do próprio device — nunca por
exigência estrutural. O protocolo servidor↔cliente (P1) só importa pra essa
fatia "✅ Sim".

P14 fica marcado como respondido por esse mapeamento — pode ser revisado
quando a Fase 9 for implementada de verdade e detalhes concretos aparecerem.

---

## Model-Agnostic: Como Funciona

O orquestrador não sabe qual modelo está rodando. Ele fala com uma trait/interface comum:

```rust
trait ModelProvider {
    fn chat(&self, messages: Vec<Message>, tools: Vec<Tool>) -> Result<Response>;
}
```

Cada provedor implementa essa trait:
- `OpenAIProvider` — API da OpenAI
- `AnthropicProvider` — API do Claude
- `GeminiProvider` — API do Google
- `LocalProvider` — modelo rodando local (ollama, llama.cpp)

---

## Canais como Adapter

Cada canal implementa:

```rust
trait Channel {
    fn send(&self, message: Message) -> Result<()>;
    fn receive(&self) -> Result<Message>;
}
```

O orquestrador não sabe de onde veio a mensagem — só processa e responde.

---

## Memória: Vault Markdown

- Arquivos `.md` no disco local do servidor
- Estrutura de pastas livre (o usuário organiza como quiser)
- Espelhado em IPFS (Filebase + Pinata)
- Busca full-text via grep/ripgrep (simples, sem precisar de banco vetorial no v1)

---

## Sub-agentes: Invocação Leve vs. Autônomos

- **Invocação leve (v1)**: agente principal chama sub-agente escopado pra tarefa específica, contexto reduzido, devolve resultado, encerra. Implementado como `DelegateTool` (`crates/warden-core/src/tool/delegate.rs`, tool `delegate_task`) — internamente é só mais um `Orchestrator` completo (mesmo `model`, mesmo `vault`, subconjunto de tools escolhido pelo chamador, nunca incluindo outro `DelegateTool`), reaproveitando 100% do loop de tool-calling existente em vez de duplicar lógica
- **Sub-agentes autônomos (fora de escopo v1)**: criam outros agentes recursivamente, precisam de fila de jobs, controle de custo, isolamento — fica pra depois