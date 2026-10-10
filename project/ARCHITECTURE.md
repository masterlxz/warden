# Decisões de Arquitetura

## Registro de Decisões

| Decisão | Opções | Status |
|---|---|---|
| Nós como capacidades (P93, Sessão 108) | Binário do nó: subcomando do `warden-server` vs binário leve novo vs desktop primeiro; tools por nó vs tools genéricas com parâmetro `node`; permissão só no nó vs nas duas pontas | **`warden-server node`**, **tools genéricas com `node`** (`list_nodes`, `node_shell`, `node_read_file`, `node_write_file`), exceto as tools MCP do nó, que viram tools próprias com prefixo por causa do schema; **duas travas, como os hosts SSH**: o nó escolhe o que oferece e o hub decide por nó (ligado, agentes, aprovação a cada chamada) ✓ (escolhas do usuário). Fatias: shell e arquivos, depois MCP do nó, depois o modelo local como provedor. Ver "Nós como capacidades" |
| Tarefas agendadas (P92, Sessão 108) | Definição no `config.toml` vs arquivo próprio; resultado numa conversa vs nota no vault vs escolha por tarefa; hora perdida roda ao voltar vs pula; tool com aprovação recusa vs pede a quem estiver on vs pré-autorizada; qual hub executa | **`[[tasks]]` no `config.toml`, resultado numa conversa da tarefa, hora perdida roda uma vez ao voltar, tool com aprovação é recusada, e cada hub tem uma chave local "executar tarefas" desligada por padrão** ✓ (escolhas do usuário). Em fatias: motor + CLI, depois telas, depois `manage_tasks` com opt-in e aprovação. Ver "Tarefas agendadas" |
| Multiusuário (P84, Sessão 107) | Root lê tudo vs root administra sem ler; restringir o agente compartilhado pelo prompt vs filtrando os dados; login por aparelho vs senha vs TruthID | **Root administra sem ler** (escolha do usuário), com backup sempre criptografado por pessoa e uma política de recuperação por workspace; **filtro por audiência das notas** (o prompt é camada extra); **todo mundo tem nome de usuário**, criado pelo root, com senha e/ou TruthID por convite. Desenho só, nada implementado. Ver "Multiusuário" |
| Rede de nós no mesmo workspace (P86, Sessão 107) | Failover entre hubs vs rede de nós; estado com um nó âncora vs serviço externo vs sem centro; fazer agora vs em etapas | **Rede de nós, sem centro (preferência do usuário), com CRDT por tipo de dado — mas adiada**: primeiro tarefas agendadas (P92) e nós como capacidades (P93), que não precisam de estado descentralizado. Ver "Rede de nós" |
| Tools do cliente na Warden API (P91, Sessão 107) | Oferecer ao modelo só as tools do cliente vs as do cliente e todas as do agente vs as do cliente e só memória/skills; guardar a `thought_signature` do Gemini no hub vs dentro do id da chamada | **As do cliente e todas as do agente, e a do cliente vale em nome repetido** ✓ (escolha do usuário; para limitar, a chave fica presa a um agente com `allowed_tools`). **A assinatura vai dentro do id** (`call_<hex>__ts_<base64url>`), sem estado no hub. Ver "Warden API" |
| Warden API (P12, Sessão 106) | Formato próprio vs compatível com a OpenAI; repassar as tools do cliente vs ignorar; salvar as chamadas como conversas vs só o gasto | **Compatível com a OpenAI (`/v1/models`, `/v1/chat/completions`, com stream) no mesmo porto do hub, tools do cliente ignoradas e nada salvo além do gasto no canal `api`** ✓ (escolhas do usuário). Chaves criadas no app (web, desktop, `warden-server api-keys`), só o hash no disco. O repasse de tools fica como pendência ligada ao P89 — ver "Warden API" |
| Roteador de APIs de IA (P79, Sessão 105) | Construir um roteador próprio vs embutir o 9Router (Node + Next.js, MIT) vs recomendar instalar por fora | **Fallback nativo entre provedores cadastrados + roteador externo opcional** ✓ (escolha do usuário). Embutir descartado (runtime Node inteiro no hub em Rust, dependência de outro projeto). Um roteador externo continua funcionando como provedor `openai_compatible`. OAuth de assinatura de consumidor fica fora do Warden (termos dos provedores). Implementado na mesma sessão, e o roteamento virou **combos com nome** (P90) — ver "Fallback entre provedores (P79) e combos (P90)" |
| Onde a memória mora vs sync (P61, Sessão 105) | Provider escolhido como fonte de leitura/escrita do agente (`Orchestrator` sobre `dyn StorageProvider`) vs disco local sempre + provider como destino de sync vs híbrido cache+fonte remota | **Disco local sempre + sync** ✓ (escolha do usuário: "local deixa mais rápido"). O agente nunca lê pela rede; o "storage" vira para onde o vault sincroniza (git ou Arweave). Consequência: o seletor de 4 cartões do desktop e a migração entre providers perdem o sentido, e `remote_node`/`RemoteNodeProvider`/`warden-node` saem (fatia 2, decisão do usuário: sem código morto). Fatia 1: o auto-sync sai do desktop para `warden_bootstrap::auto_sync::SyncRunner` e passa a rodar também no hub standalone, com tela na web |
| Framework desktop | Tauri vs Electron vs nativo | **Tauri** ✓ — reaproveita stack Rust/TS já usada no TruthID |
| TLS do hub (P36 fatia 2, Sessão 96) | Cert autoassinado + fingerprint no QR (extensão não consegue fixar) vs certs do Tailscale (`tailscale cert`, Let's Encrypt pro nome MagicDNS) vs `ws://` só em loopback/LAN pra extensão; com TLS ligado: mesma porta aceitando `ws://` só pro `Discover` vs TLS sem exceção | **Certs do Tailscale + mesma porta, `ws://` só pro Discover** ✓ (escolha do usuário). `warden-server serve --tailscale-cert` (nome via `tailscale status --json`, `tailscale cert` na subida + renovação diária) ou `--tls-cert/--tls-key[/--tls-host]` genérico; `ReloadingCertResolver` relê os PEM quando o mtime muda (renovação sem restart). O hub olha o 1º byte (`0x16` = TLS): TLS segue o protocolo completo; `ws://` puro só faz upgrade no path `/discover` (`warden_server_protocol::tls::DISCOVER_PATH`) e só responde `DiscoverAck{secureUrl}` — qualquer outro path leva `426 Upgrade Required` **antes** do `Hello`, então um cliente mal configurado nunca manda a chave em texto puro. Clientes Rust verificam contra `webpki-roots` (sem pinning). Provider do rustls escolhido explicitamente (`ring`): o workspace compila `ring` e `aws-lc-rs` juntos (via `reqwest`/`rmcp`) e aí o rustls entra em pânico se tiver que escolher sozinho. Sem TLS configurado, nada muda (`ws://` como antes). **Fatia 3 (Sessão 96, continuação)**: no desktop, só o toggle do Tailscale (cert manual fica na CLI); mobile e extensão mantêm host + porta + um switch "Use TLS" (em vez de virar um campo de URL único), com o token por device ainda indexado por `host:port` — num hub TLS o host é o nome `.ts.net`. |
| Auth do hub: token por device (P36 fatia 1, Sessão 94) | Só bloquear `Revoked` no `Hello` (contornável trocando o `device_id`, que é escolhido pelo cliente) vs token por device emitido no pareamento; TLS antes ou depois | **Token por device, TLS depois** ✓ (escolha do usuário). A `auth_key` compartilhada virou **chave de pareamento**: `Hello{deviceToken?}` / `HelloAck{deviceToken?}` (campos opcionais, wire antigo continua parseando). `PairingStore::authenticate` decide em ordem: `Revoked` → sempre recusa; token bate com o hash guardado → aceita, status intacto; senão, chave de pareamento certa → emite token novo — **e, se aquele `device_id` já tinha token, volta pra `Pending`** (quem só tem a chave não herda o `Approved` de outro device alegando o id dele; um registro de antes dos tokens mantém o status, pra o upgrade não desaprovar todo mundo); senão `AuthError`. Só o SHA-256 do token vai pro `devices.json`. Revogar agora **derruba a conexão aberta**: cada conexão relê o registro a cada 5s (`DEFAULT_REVOCATION_CHECK_INTERVAL`; o revoke vem de outro processo — CLI ou Workspace do desktop — então o arquivo é o único sinal), manda `AuthError{"device revoked"}` e fecha. Rotacionar a chave (`--auth-key`/"Gerar nova chave" + reiniciar) só afeta pareamentos novos. Cliente: `DeviceTokenStore` (`device_tokens.json` no config dir, chave `url\|device_id`) pra `warden-node`/`RemoteNodeProvider`; `shared_preferences` por `host:port` no mobile; `chrome.storage.local.deviceTokens` por `host:port` na extensão. **Achado no caminho**: `handle_connection` nunca terminava — `tool_channel`/o `Orchestrator` da conexão seguravam clones do `tx`, então o `writer_task.await` final esperava pra sempre e o socket não fechava do lado do servidor; corrigido soltando os clones antes do await |
| Extensão no Firefox (P69 item 2, Sessão 93) | Camada de abstração + um build vs implementação paralela; um manifest universal (Chrome e Firefox ignoram as chaves um do outro) vs um manifest por navegador | **Mesmo código, um manifest por navegador, dois builds** ✓ — `manifestFor(target)` em `extension/manifest.config.ts` + `vite build --mode firefox` → `dist-firefox/` (crxjs `browser: "firefox"`). Manifest universal descartado: cada navegador emitiria avisos de chave/permissão desconhecida, e o crxjs só gera `background.scripts` com `browser: "firefox"` de qualquer jeito. A divergência de runtime ficou contida em `background/platform.ts` (abrir o painel; descoberta de hub na LAN, que não tem como existir no Firefox sem `system.network`), por checagem de feature. Firefox 140+ |
| Histórico da conversa ao reconectar (P40, Sessão 93) | Empurrar o histórico dentro do `HelloAck` vs par request/response explícito | **`ClientMessage::RequestHistory{requestId, limit?}` → `ServerMessage::History{requestId, messages}`/`HistoryError`** ✓ — só quem pede recebe (extensão/`warden-node` inalterados, cliente antigo nunca vê `type` desconhecido), `limit` corta pelo fim (mobile pede 100, por causa de anexos base64). DTO próprio `HistoryMessage`/`HistoryRole` em `warden-server-protocol` (o `ChatRole` do `warden-bootstrap` criaria ciclo). Respondido inline no loop de leitura do `server.rs` (`history.rs::handle_history_request`, função pura) — como o `Chat` roda em task separada e só salva ao terminar, um turno enviado depois do pedido nunca aparece duplicado no histórico. No mobile, `ChatTranscript` insere o histórico antes de qualquer coisa enviada enquanto ele chegava |
| Framework mobile | Tauri Mobile vs Flutter | **Flutter** ✓ (revertido de Tauri Mobile, Sessão 50 continuação) — usuário priorizou maturidade geral e suporte a iOS; ver nota detalhada abaixo (seção "Mobile: troca de Tauri Mobile pra Flutter") |
| Topologia de rede | Estrela vs Malha P2P | **Estrela** ✓ — servidor central, clientes se conectam. **Refinado 2026-08-02**: servidor é opcional, só entra quando a feature exige coordenação entre múltiplos nodes — ver nota abaixo e P14 em `PENDING.md` |
| Memória | Markdown vault (Obsidian) vs banco vetorial | **Markdown vault** ✓ — portátil, legível, versionável |
| Backup | IPFS (Filebase/Pinata) vs S3 vs Arweave via TruthID | **Superado — ver linha "Sync descentralizado (Fase 4)" abaixo** (Sessão 50). Registro histórico: a decisão original era IPFS (Filebase/Pinata), mesmo padrão que o TruthID usava antes de migrar pra Arweave (P24 em `PENDING.md`) |
| Model-agnostic | Camada de abstração vs hardcoded | **Camada de abstração** ✓ — suporta OpenAI, Anthropic, Gemini |
| WhatsApp | Baileys (Node.js) vs nativo Rust | **Baileys (sidecar Node)** ✓ — não vale reescrever em Rust |
| Telegram | Bot API HTTP vs MTProto | **Bot API HTTP** ✓ — mais simples, sem risco de ban |
| Protocolo servidor↔cliente | gRPC vs WebSocket vs HTTP | **WebSocket + protocolo JSON próprio** ✓ (Sessão 50). Rodando dentro do túnel já criptografado do Tailscale (a malha resolve conectividade/segurança de transporte — o WS não precisa reimplementar isso). Motivo: a Fase 9 (9.4/9.5) exige o servidor **empurrar** "execute esta tool" pro cliente certo e receber o resultado de volta pela mesma conexão — duplex por natureza, que WS cobre com uma conexão persistente simples; gRPC faria o mesmo via streaming bidirecional, mas exigiria protobuf/`tonic`/codegen nos dois lados (incluindo o Tauri mobile) só pra ganhar tipagem forte que o projeto já resolve com JSON simples nos outros dois protocolos internos que existem hoje (linha JSON no sidecar do WhatsApp, JSON-RPC do MCP). **Correção de uma suposição antiga**: a nota original desta linha assumia "reaproveitar o relay stateless por WS do TruthID" — conferido o SDK real do TruthID (`docs/docs/sdk/dart.md`, `TruthIDRequester`) e isso está incorreto: o mecanismo de lá é local-network sweep + IPFS/IPNS dead-drop, **sem relay/servidor nenhum** ("No relayer, no TruthID server, no polling endpoint for you to host") — resolve pareamento sem VPN compartilhada, um problema que o Warden não tem (já assume Tailscale). Nada de lá foi reaproveitado. Decisão fecha P1 e desbloqueia tanto a 7.2 (mobile↔servidor) quanto a 9.2 — mesmo protocolo serve as duas, só variando o tipo de mensagem JSON trocada. **Ainda em aberto, fora do escopo desta decisão**: formato exato das mensagens JSON (schema por tipo de evento), quem assume o papel de "servidor" (desktop sempre ligado? processo headless dedicado?) e autenticação da conexão WS em si — ficam pra quando a 7.2/9.2 forem implementadas de fato |
| Implementação da 9.2 (crate/dependência/schema) | Onde vive o código do protocolo; `tokio-tungstenite` vs promover `axum` (já dev-dependency em `warden-core`); schema exato das mensagens | **Novo crate `crates/warden-server`** ✓ (Sessão 50), bin **+** lib (diferente do `warden-mcp-server`, que é só bin) — o lado *client* (`ServerConnection`) já nasce reutilizável pra Fase 7.2 (desktop/mobile como cliente) sem redesenhar o wire format depois. **`tokio-tungstenite`, não `axum`**: o `ws` extractor do `axum` puxa o mesmo `tokio-tungstenite` por baixo mais a pilha HTTP inteira (tower/hyper/matchit) pra um servidor com um endpoint só e zero semântica HTTP — quem barra alcançabilidade é o Tailscale (9.1), não middleware HTTP; mesmo raciocínio já usado pro catcher de redirect OAuth (não promover `axum` a dependência de produção por um listener local de propósito único, ver decisão de MCP OAuth acima). **Deliberadamente sem `warden-bootstrap`/`warden-core`** — zero superfície de tool dispatch nesta peça (isso é 9.4), então um `Orchestrator` aqui seria peso morto; entra quando a 9.4 precisar de fato despachar um `Tool::call` pro cliente certo. **Schema** (`src/protocol.rs`): `ClientMessage`/`ServerMessage`, dois enums (um por direção, mesmo estilo do IPC do sidecar do WhatsApp — não JSON-RPC, isso é específico do MCP), `#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]` — `rename_all` sozinho só renomeia o nome da variante/tag, os campos dentro de cada variante exigem `rename_all_fields` à parte (achado real ao rodar o teste de round-trip JSON, que falhou na primeira tentativa por causa disso). `ClientMessage::{Hello{device_id,device_name,auth_key}, Ping{nonce}, Goodbye{reason}}`, `ServerMessage::{HelloAck{server_name}, AuthError{reason}, Pong{nonce}, Goodbye{reason}}` — falha de auth é um `AuthError` tipado (não um close silencioso), seguido de um WS `Close` código 1008 (policy violation); sem campo de versão de protocolo (o próprio `type` já é o ponto de extensão). **Chave de auth**: sem `FileConfig` — resolução de um campo só (`WARDEN_SERVER_AUTH_KEY` env, vence sobre `--auth-key`) inline no `main.rs`, mesmo *padrão* de `resolve_secret` sem puxar `warden-bootstrap` inteiro por um campo. Porta default **7420** — sem convenção prévia no projeto, escolhida só pra não colidir com a porta do dev server do Tauri (`1420`, `desktop/src-tauri/tauri.conf.json`). **Verificado com teste de integração real** (`crates/warden-server/tests/handshake.rs`, mesmo espírito não-mockado de `crates/warden-core/tests/mcp_http.rs`): servidor real bindado em `127.0.0.1:0` (porta OS-assigned), client real conectando — handshake com chave certa (`HelloAck`), handshake com chave errada (`AuthError` + conexão fechada, confirmado que a conexão não completa), heartbeat com múltiplos ping/pong na mesma conexão. `cargo build/test/clippy --workspace` limpos. **Sem Tailscale real pra testar sobre a malha de verdade** (mesma lacuna já aceita em outras partes do projeto por falta de infra externa no ambiente de dev, ex. P29/P30/P31) — aceito, registrado em `PENDING.md` |
| Fase 9.1 — o que "todo nó na mesma subnet" realmente significa | Integração nativa com Tailscale (código específico no Warden) vs descoberta de LAN + Tailscale como infra opcional do usuário | **Redefinida pelo usuário (Sessão 69, continuação)**: 9.1 nunca foi "integrar com Tailscale" — é o Warden conseguir **descobrir sozinho outros dispositivos/hubs na rede local**, pra desktop/mobile/extensão não precisarem de host/porta digitados à mão. Tailscale continua 100% infra do próprio usuário, fora do código do Warden — quem configurar Tailscale nos dispositivos ganha uma "rede externa" de graça, porque a conexão WS já existente não sabe nem precisa saber que está passando por um túnel (mesma decisão que sessões anteriores já tinham registrado: "9.1 Tailscale segue como configuração de infra, não trabalho de código"). **Fatia 1 implementada** (protocolo + servidor + sweep em Rust + desktop): `ClientMessage::Discover`/`ServerMessage::DiscoverAck{server_name}` novos em `crates/warden-server-protocol/src/protocol.rs` — sem `auth_key`/`device_id`, de propósito (o objetivo é achar um hub *antes* de conhecer a credencial dele; a resposta só revela um nome de exibição, nunca dado sensível, mesmo nível de exposição que mDNS/SSDP já aceitam pra impressoras/Chromecasts na mesma rede). `crates/warden-server/src/server.rs::handle_connection` trata `Discover` como primeiro frame **antes** da checagem de `Hello`/`auth_key` — responde e fecha, sem nunca chamar `PairingStore::record_seen` (uma sonda nunca vira um "dispositivo conectado"). `Server` ganhou um campo `server_name` de verdade (antes o `HelloAck` respondia com o literal fixo `"warden-server"`) — resolvido via `--server-name`/`WARDEN_SERVER_NAME`/hostname/literal fixo, mesma cadeia de fallback que `crates/warden-sync/src/pairing/join.rs::device_name()` já usava pro lado cliente. Sweep em si (`crates/warden-server-protocol/src/discovery.rs::discover_hubs`/`discover_hubs_on`) reaproveita `warden_truthid::lan::candidate_hosts()` (mesmo /24-por-interface do pareamento de vault, Fase 4.4) — nova dependência de `warden-truthid` no `Cargo.toml` do `warden-server-protocol` (sem ciclo, `warden-truthid` não depende de nada rio-abaixo). Duas diferenças deliberadas em relação ao sweep de pareamento (`pairing/join.rs`) que serviu de modelo: nenhum código secreto é trocado (qualquer host pode responder) e a varredura **não** re-tenta até um deadline longo — é uma sondagem pontual de ~1-2s, sem o "o celular pode não estar pronto ainda" que justificava o retry ali; um "Refresh" manual cobre o caso de querer tentar de novo. `handle_connection` cresceu pra 8 argumentos nesse meio-tempo — refatorado num `ConnectionContext` (`#[derive(Clone)]`, agrupando tudo que não é por-conexão) antes que o clippy `too_many_arguments` disparasse. Desktop: `workspace_cmds::discover_hubs` (porta fixa 7420, limitação conhecida — ver `PENDING.md`) alimenta um botão "Procurar hubs na rede" na seção de pareamento por QR do `WorkspaceView.tsx`; clicar num resultado preenche só o campo Server URL, a Auth key continua manual por design (a sonda nunca a revela). Verificado com testes reais (`Server::bind` de verdade em `127.0.0.1:0`, não mockado) em `crates/warden-server/tests/handshake.rs` + `crates/warden-server-protocol/src/discovery.rs`; `cargo build/test/clippy --workspace` e `tsc`/`npm run build` do desktop limpos. **Fatia 2 (mesma sessão, continuação) — mobile**: `crates/warden-mobile-bridge` ganhou `warden-server-protocol` como dependência nova (sem ciclo) e `api/discovery.rs::bridge_discover_hubs(port)` — mesmo formato `pub fn` simples + `tokio::runtime::Runtime` próprio (`OnceLock`) que `api::sync` já usava, chamando a mesma `warden_server_protocol::discover_hubs` que o desktop chama, não uma reimplementação Dart do sweep. `mobile/lib/screens/connection_screen.dart` ganhou um `IconButton` (ícone `wifi_find`) ao lado do scanner de QR — abre um `showModalBottomSheet` com loading/lista/vazio (`_DiscoveredHubsSheet`), tap num hub preenche só host/porta, auth key continua manual. Bindings Dart regeneradas via `flutter_rust_bridge_codegen generate` (reinstalado nesta sessão via `cargo install --version 2.13.0`, mesma versão do `flutter_rust_bridge` já fixada no `pubspec.yaml`/`Cargo.toml` — não persistia no ambiente, precisou reinstalar; o codegen por sua vez auto-instalou `cargo-expand` como dependência dele mesmo). **Limitação aceita desta fatia**: sem build/teste num emulador Android real — `cargo-ndk` (só necessário pra compilar o `.so` cross-compilado) também não persistiu, e reinstalá-lo só pra confirmar visualmente o botão não foi julgado necessário já que a fatia 1 já provou o mecanismo de sweep de ponta a ponta contra um `warden-server` real (a mesma função Rust que esta fatia só expõe via FFI). Verificado com `cargo build/test/clippy --workspace` e `flutter analyze`/`flutter test` (44 testes) limpos. **Fatia 3 (mesma sessão, continuação) — extensão de navegador**: como ela roda só em TypeScript e não pode chamar Rust, `extension/src/background/discovery.ts` reimplementa o mesmo sweep, falando o protocolo `Discover`/`DiscoverAck` idêntico contra o servidor inalterado — `extension/src/protocol/messages.ts` (a cópia à mão de `protocol.rs` que a extensão já mantinha) ganhou as duas variantes novas. `candidateHosts()` usa `chrome.system.network.getNetworkInterfaces()` (única forma de uma extensão aprender sua própria sub-rede — sem equivalente a `if-addrs` nesse contexto) pra montar o mesmo /24-por-interface; `probeOne()` abre um `WebSocket` curto por candidato (`Promise`+`setTimeout` no lugar do `tokio::time::timeout` do Rust); `mapWithConcurrency()` (helper novo, sem dependência) limita a concorrência a 50 em voo, mesmo valor do lado Rust — evita abrir as 254 conexões de um /24 de uma vez só no service worker. **Achado no caminho**: a permissão `system.network` (adicionada ao `manifest.config.ts`) não está coberta pela versão de `@types/chrome` (`0.3.0`) que o projeto já usava — só tem `system.cpu`/`memory`/`storage`/`display`, não `system.network`. Resolvido com uma declaração ambiente local (`extension/src/types/chrome-system-network.d.ts`, mesmo formato de overload Promise/callback que `system.storage` já usa no pacote de tipos) em vez de recorrer a `any`/`@ts-ignore`. `ConnectionForm.tsx` ganhou um botão "Procurar hubs na rede" + lista inline clicável (`.hub-list`/`.hub-item` novos em `App.css`, mesma linguagem visual do `workspace-device-row` do desktop) — preenche só host/porta, nunca a auth key, mesma linha de segurança das duas fatias anteriores. Roteamento popup↔background: `PopupRequest` ganhou `{type:"discoverHubs"}`, `background/index.ts::handleRequest` despacha pra `discoverHubs()` e devolve `DiscoverHubsResponse{ok,hubs,error?}`. Verificado com `npx tsc --noEmit`/`npm run build` limpos (`dist/manifest.json` conferido manualmente com a permissão nova); sem Playwright/Chrome real disponível neste ambiente pra recarregar a extensão de verdade e clicar o botão — mesma lacuna que P67/P68 já aceitaram pra outras partes da extensão, registrada de novo em `PENDING.md` P70. **Fecha as três fatias da Fase 9.1 por completo, registrado em `PENDING.md` P70**: só ficam as limitações menores (porta fixa 7420 em todas as três, build/teste Android real da fatia 2, verificação manual da extensão num Chrome/Brave real) |
| Fase 9.8 — app desktop embute o próprio `warden-server` | O hub continuar sendo sempre um processo à parte vs o mesmo app desktop virar o hub via um toggle nas Configurações | **App desktop embute** ✓ (Sessão 69, continuação — pedido explícito do usuário, "quero que o servidor seja o mesmo aplicativo"). `crates/warden-server/src/server.rs::Server` ganhou `serve_until(shutdown: impl Future<Output = ()>)` — o `serve()` de sempre virou `self.serve_until(std::future::pending()).await`, então o binário standalone não muda de comportamento; `tokio::select!` entre `listener.accept()` e o `shutdown` deixa o toggle "Desligar" liberar a porta de verdade em vez de deixar uma task rodando pra sempre. `warden_bootstrap::EmbeddedServerConfig{enabled, port, auth_key, server_name}` novo (`FileConfig.embedded_server`) — **host deliberadamente não é campo nenhum** (sempre `0.0.0.0`, já que o ponto inteiro é ser alcançável; só a porta é escolha real, exatamente o que o usuário pediu). **Auth key gerada, nunca digitada**: `generate_auth_key()` (32 bytes de `rand::rngs::OsRng`, hex) — abrir uma porta de rede de verdade com uma chave fraca/vazia seria um risco real; `rand` novo como dependência do `warden-bootstrap` (nenhuma alternativa reaproveitável do `warden-truthid` sem forçar um contexto de protocolo específico, ver comentário no código). `resolve_server_name` (antes só privada em `warden-server/src/main.rs`) virou função pública de `warden-server` (`server.rs`) pra o desktop reaproveitar a mesma cadeia de fallback (nome explícito → `WARDEN_SERVER_NAME` → hostname → literal) sem duplicar lógica. Novo `desktop/src-tauri/src/server_cmds.rs`: `AppState` ganha `embedded_server: Mutex<Option<EmbeddedServerHandle>>` (shutdown sender + endereço bindado + nome); `start_embedded_server_inner` (compartilhada entre o comando Tauri e o auto-start em `run()`) clona o mesmo `Orchestrator` que já serve o chat local do desktop (`state.orchestrator.lock().unwrap().clone()`, já era o padrão usado por `send_message`) e usa os mesmos `default_server_conversations_dir`/`default_server_devices_path` que o binário `warden-server` standalone já usaria — o `WorkspaceView.tsx` (Fase 9.6/9.7), que já assumia "hub na mesma máquina que o desktop", passa a enxergar o hub embutido sem nenhuma mudança própria. **Confirmado com o usuário antes de codar**: uma vez ligado, o servidor volta a subir sozinho em todo lançamento do app (`embedded_server.enabled` persistido) — não é um toggle só-da-sessão, mesmo espírito "serviço sempre no ar" do Jellyfin que motivou o pedido. `save_embedded_server_config` (porta/chave/nome) nunca mexe em `enabled` — só `start_embedded_server`/`stop_embedded_server` fazem isso, editar a porta não liga/desliga nada sozinho. UI nova em `WorkspaceView.tsx` (`EmbeddedServerSection`, topo da tela, antes do pareamento por QR): toggle Ligar/Desligar, porta (só editável parado), nome opcional, campo de auth key reaproveitando o `ApiKeyField` que já existia só dentro de `SettingsView.tsx` (exportado nesta sessão) + botão "Gerar nova chave". **Verificado com um teste de ponta a ponta real, não mockado**: `Orchestrator` de verdade via `bootstrap()` (config/vault descartáveis, chave de API falsa nunca de fato chamada), `AppState` real, `start_embedded_server_inner` real, e um `ServerConnection`/`discover_hubs_on` reais conectando por um socket TCP de verdade — `Hello`+`Ping` respondidos e `Discover` achando o hub com o nome certo. `cargo build/test/clippy --workspace` e `tsc`/`npm run build` do desktop limpos. **Fora de escopo, por decisão explícita do usuário durante a pesquisa desta sessão**: acesso de fora da LAN (tipo Jellyfin, redirecionar a porta no roteador) já funciona sem nenhum código — é 100% configuração do roteador do usuário, orientação passada em conversa, não uma feature; TLS pra essa exposição direta à internet continua em aberto (P36) |
| Fase 4.7 — sync automático ao reconectar (P71) | Detecção real de evento de reconexão vs checagem periódica; auto-push também, ou só auto-pull | **Auto-pull periódico no desktop** ✓ (Sessão 69, continuação — pedido explícito do usuário, confirmado antes de codar: checagem periódica em vez de detecção real de evento de rede, e só desktop nesta fatia, mobile fica pra depois). **Achado que redesenhou o escopo**: os dois backends de sync não são igualmente automatizáveis — Arweave/TruthID (`SyncEngine`, o único com UI no desktop) tem `pull()` totalmente automatizável (`crates/warden-sync/src/pull.rs`: já faz uma consulta GraphQL barata, `latest_tx_by_owner`, antes de baixar qualquer coisa — `pull()` repetido sem nada novo é barato), mas `push` **não dá pra automatizar**: `finish_push` bloqueia esperando aprovação física no celular do TruthID, trava de segurança proposital, não uma lacuna de implementação. O backend git (P63, `GitSyncEngine`) seria totalmente automatizável nos dois sentidos (só token + rede), mas **nunca foi ligado ao desktop** — só existe no CLI (`/sync git push`/`pull`). Por isso esta fatia entrega só auto-pull via Arweave; auto-push fica pra quando o git for wireado no desktop (P71). `desktop/src-tauri/src/sync_cmds.rs::spawn_auto_pull(app, vault_path, config_path, secrets_path, manifest_path)` — chamada uma vez de dentro de um `.setup(|app| {...})` novo no `tauri::Builder` (primeiro hook desse tipo no app; antes o `run()` só tinha um bloco de auto-start síncrono pro servidor embutido, Fase 9.8). Constrói seu próprio `SyncEngine` independente (não tem `Clone`, mas é barato reconstruir — `Vault::new` + 3 `PathBuf` + `ArweaveClient::new_default()` — os métodos só leem/escrevem os mesmos arquivos em disco que o `AppState.sync` original, então dois engines apontando pro mesmo path nunca desincronizam entre si) e roda um loop `tokio::time::interval(5 min)` — o primeiro `tick()` de um `interval` resolve na hora, então o próprio boot do app já cobre "acabei de reconectar" sem precisar de uma chamada separada de startup. Gate `is_initialized()` (só checa se o arquivo de secrets existe, sem rede) pula silenciosamente quando o sync nunca foi configurado neste device; quando configurado, chama `pull()` de verdade e só emite `app.emit("auto-sync-pulled", ...)` (mesmo padrão de `pairing-completed`/`sync_cmds.rs::pairing_start` já usava) se algo realmente mudou (`files_written`/`files_deleted`/`config_updated` > 0) — silêncio total quando já está tudo atualizado. `SyncView.tsx` ganhou um `useEffect` novo escutando esse evento, reaproveitando `refreshStatus()`/a classe `settings-success-banner` já existentes, nenhum componente novo. **Verificado com testes reais, não mockados**: um teste prova que o gate de "nunca inicializado" nunca chega a chamar `pull()` (apontado pra um endereço não-roteável de propósito — chamaria a rede e travaria/erraria se o gate não bloqueasse antes); outro constrói um device já *pareado* de verdade (secrets + manifest com `owner_address` escritos direto em disco via `manifest::save_secrets`/`save_manifest` — `pull()` erra cedo num device sem `owner_address`, então só rodar `init_fresh()` não bastava pra exercitar esse caminho) e confirma que `pull()` alcança um gateway HTTP fake local de verdade (mesmo idioma de `crates/warden-sync/tests/fake_arweave_gateway.rs`) e completa sem erro. Não repliquei o round-trip completo de push→pull com conteúdo real (isso já está coberto por `warden-sync`'s próprio `engine_lifecycle.rs`, que simula um telefone TruthID fake e não pode ganhar um segundo teste no mesmo binário sem colidir na porta LAN real que ele bind — o que estava sob teste aqui era o mecanismo do loop/gate, não a mecânica de `pull()` em si). `cargo build/test/clippy --workspace` e `tsc`/`npm run build` do desktop limpos |
| Sync descentralizado (Fase 4) — quem paga/publica no Arweave | Carteira Arweave própria do Warden vs usar a carteira do TruthID como pagador | **Carteira do TruthID como pagador** ✓ (Sessão 50) — decisão explícita do usuário: "eu quero que o truthid seja o local onde possa ser cobrado essas taxas". Investigação no código real do TruthID (não só docs) confirmou que a carteira Arweave de lá (RSA-4096 JWK) é **independente** do lado EVM/smart-account — dá pra usar o mecanismo de pagamento sem herdar nenhuma infra EVM/bundler. O caminho é o `TruthIDRequester.pin()` que o SDK Dart já expõe pra apps terceiros: QR → o app TruthID escaneia/aprova → cifra em trânsito → decifra → publica no Arweave com a própria carteira → devolve `PinResult{cid: "ar://<txid>", ...}`. **Tudo cifrado antes de sair do device** (decisão explícita e não-negociável do usuário, "vamos criptografar tudo certo?") — o TruthID nunca vê o conteúdo em texto puro, só serve de "correio pago". Escopo do que sincroniza: vault + `config.toml` inteiro (chaves de API, providers, agentes, `mcp_servers` — este último incluído por decisão explícita, mesmo nível de sensibilidade das chaves de API); conversas ficam de fora. **Limitações reais do `pin()` que moldam o design daqui pra frente**: sem tags customizáveis no Arweave (todo pin de terceiro leva só `App-Name: TruthID` fixo — inviabiliza descoberta "latest by tag" via GraphQL, precisa de outra solução pro ponteiro de versão mais recente) e sem batching (uma aprovação física por chamada — N arquivos mudados precisam virar um blob só por sync). Ver P37 em `PENDING.md` pro resto das decisões ainda em aberto (formato do manifesto, ponteiro de versão, identidade que deriva a chave de cifra do vault) |
| Sync alternativo via remote git próprio (Fase 4, backend alternativo ao Arweave) | Blob único cifrado por push (reusa `bundle.rs` tal como é, resolve o device-novo via replay de histórico de commits no `pull`) vs árvore de arquivos cifrados individualmente no working tree (nomes/estrutura ofuscados) | **Blob único cifrado + git só como transporte/histórico ordenado** ✓ (decisão de arquitetura, 2026-09-10; implementado na Sessão 62 — `crates/warden-sync/src/git.rs`) — árvore por nota foi descartada por vazar nomes/estrutura do vault (ex. `notas/terapia/...`) pro operador do git mesmo com conteúdo cifrado; o bug real que motivou a ideia (device novo só recebe o último diff, nunca o vault reconstruído — ver `pull.rs`/P37) é resolvido fazendo `pull` percorrer e reaplicar o histórico de commits em ordem, não reestruturando o modelo de dados. Motor irmão de `warden-sync` (`git.rs` ao lado de `arweave.rs`), reaproveitando `bundle.rs`/`diff.rs`/`manifest.rs`/`pairing/` sem mudança — não uma implementação de `StorageProvider` (mesmo raciocínio de P61: a trait é CRUD síncrono por arquivo, não serve pra publicar um bundle versionado). Shell-out pro binário `git` do sistema no v1 (não `git2`/OpenSSL, não `gix` ainda — ver justificativa em P63/`PENDING.md`), desktop+CLI só, HTTPS+token só; SSH, UI de Settings e mobile ficam pra v2. Ver P63 em `PENDING.md` pro resto dos detalhes |
| `warden-truthid` (Sessão 50) — implementação do cliente do protocolo `pin()` | Onde vive o código; `secp256k1` nativo (bindings C) vs `k256` (RustCrypto puro Rust); como testar sem hardware | **Novo crate `crates/warden-truthid`**, só lib por enquanto (sem consumidor ainda, mesma situação do lado *client* do `warden-server`) — replica byte a byte o protocolo real do SDK Dart do TruthID (`sdk/dart/lib/src/requester.dart` + `internal/ecies.dart` + `internal/pin_content_cipher.dart`), já que não existe spec do protocolo fora do próprio código-fonte. **`k256` em vez de `secp256k1`/`libsecp256k1`** — evita mais uma dependência nativa pra cross-compilar (o build Android da 7.1 já mostrou como isso dói, `-laaudio` do `cpal`), consistente com o resto do ecossistema RustCrypto já usado (`aes-gcm`, `hkdf`, `sha2`). Módulos: `protocol.rs` (`QrPayload`/`PinResult`, espelham os campos do Dart exatamente), `crypto.rs` (fase 1: HKDF-SHA256 sobre o `session_id` cru, salt `"TruthID Pin Content"`, info `"content-key-v1"`, AES-256-GCM, layout `nonce(12)\|\|ciphertext\|\|tag(16)`; fase 2: ECIES secp256k1 — ECDH + SHA-256 puro (sem HKDF) + AES-256-GCM, layout `ephemeral_pubkey(33 comprimida)\|\|nonce(12)\|\|ciphertext\|\|tag(16)`), `lan.rs` (`candidate_hosts()` via `if-addrs`, varredura de `/24` por interface não-loopback × porta fixa `48050-48054`, mesma faixa usada pelos dois lados no Dart já que o QR não carrega IP/porta), `requester.rs` (`PendingPin::begin`/`qr_payload_json`/`run`, espelha a forma do `PendingRequest` do Dart — QR pronto na hora, resultado é assíncrono). **`run_with_hosts` como API pública adicional** (não só um hack de teste) — permite um chamador que já sabe o IP do celular por outro canal pular a varredura; é o que os testes usam também, pra não varrer a LAN real do container (que tem interfaces de verdade, `wlp0s20f3`/`docker0` — uma varredura de `/24` completa nelas seria lenta e não-determinística num teste automatizado). **Testado sem hardware nenhum**: vetor conhecido de RFC 5869 (HKDF-SHA256) rodado direto contra a primitiva usada; round-trip de cada camada de cifra; um "celular fake" (servidor `axum` de teste, só como dev-dependency, mesmo padrão de `crates/warden-core/tests/mcp_http.rs`) implementando os mesmos dois endpoints HTTP single-shot do `RemoteSignerLanServer` real (`PUT /session/:id/content`, `GET /session/:id`), provando o fluxo `PendingPin` inteiro (fase 1 → varredura → fase 2 → decifra ECIES → parse de `PinResult`) de ponta a ponta. `cargo build/test/clippy --workspace` limpos (9 testes unitários + 1 de integração). **Nunca testado contra o app TruthID real** — por pedido explícito do usuário nesta sessão (sem hardware), registrado como P38 em `PENDING.md`; maior risco de interoperação é a convenção exata do "ECDH secret" do pacote Dart `elliptic` (assumida como coordenada X crua, mesma convenção do `k256`, não confirmada rodando Dart de verdade). **Atualizado 2026-09-10 (Sessão 59, continuação 3)**: essa convenção foi confirmada rodando o SDK Dart real, não só lendo o código — não existe SDK oficial do TruthID em Rust (só Dart/Python/Ruby/TypeScript), então a validação foi feita instalando o Dart SDK e executando `elliptic-0.3.12/lib/src/ecdh.dart`'s `computeSecret` + `sdk/dart/lib/src/internal/{hkdf,pin_content_cipher}.dart` com entradas fixas via um script descartável (apagado depois, `git status` do repo do TruthID confirmado limpo). Resultado: idêntica — X-coordinate big-endian zero-padded a 32 bytes, exatamente o que `k256::ecdh::diffie_hellman(...).raw_secret_bytes()` já produz. 2 testes novos em `crypto.rs` travam os vetores reais como regressão permanente. Ver `PENDING.md` P38 pro que continua em aberto (teste contra hardware físico) |
| `warden-sync` (Sessão 54) — motor de sync completo (P37) | Formato do manifesto/bundle; como descobrir a "última versão" sem tags no Arweave; mecanismo de pareamento pra compartilhar a chave do vault | **Novo crate `crates/warden-sync`**, consumido por `desktop/src-tauri` e `warden-cli` (não por `warden-bootstrap`, que é `deny_unknown_fields` e tem ciclo de vida diferente). `manifest.rs`: `SyncSecrets` (`device_id` + `vault_key` de 32 bytes, `~/.config/warden/sync_secrets.json`, nunca reescrito) e `SyncManifest` (hash sha256 por arquivo do vault + hash do `config.toml` + `owner_address`/`last_tx_id`/`manifest_counter`, `sync_manifest.json`, reescrito a cada push/pull). `diff.rs`: `Vault::list_all_files()` novo em `warden-core` (todos os arquivos, não só `.md` — `list_files` original ficou intocado) comparado hash a hash contra o manifesto. `bundle.rs`: envelope JSON (arquivos mudados em base64 + deletados + config se mudou) cifrado como um blob único via `warden_truthid::crypto::encrypt_pin_content`/`decrypt_pin_content` **reaproveitados diretamente** (só a chave HKDF muda — salt/info novos, `"Warden Sync Bundle"`/`"bundle-content-key-v1"`, deliberadamente diferentes do contexto do TruthID). `arweave.rs`: cliente GraphQL simples (`reqwest`) — **decisão-chave**: como `pin()` não permite tags customizadas, "última versão" é descoberta consultando `transactions(owners:[...], sort:HEIGHT_DESC)` pelo **endereço da carteira do TruthID** (aprendido uma vez, via `transaction(id:...){owner{address}}` sobre a própria tx do primeiro push, e propagado depois via pareamento) — resolve o gap que a Sessão 50 tinha deixado em aberto sem precisar de nenhum ponteiro copiado à mão. `push.rs`/`pull.rs`: `begin_push`/`run_push` separados (mostra o QR antes de bloquear no telefone), `pull` compara hashes contra o manifesto ANTIGO antes de sobrescrever pra avisar sobre mudança local perdida (last-write-wins é a política de conflito do v1, sem merge de 3 vias). **Pareamento é um protocolo novo, não o QR do TruthID**: código curto (8 chars, alfabeto sem `0/O/1/I/L`) + LAN, sem câmera — reaproveita só os primitivos genéricos de `warden-truthid` (`crypto::{generate_ecies_keypair,ecies_encrypt,ecies_decrypt}`, `lan::candidate_hosts()`), com uma faixa de portas própria (`48070-48074`, distinta de `LAN_PORTS` do TruthID) e um listener de verdade (`pairing::host`, idioma `tokio_tungstenite`/`accept_async` igual ao `warden-server` de produção — não `axum`, que no projeto só existe em testes) já que `warden-truthid` nunca teve um listener real, só o cliente que varre. **Bug real achado pelo teste de ponta a ponta**: a primeira versão do pareamento também propagava `last_tx_id` do host pro dispositivo que entra — isso fazia o primeiro `pull` do novo dispositivo achar que "já estava atualizado" sem nunca ter baixado nada, já que `manifest.last_tx_id` batia com o tx mais recente sem `vault_files` correspondente. Corrigido removendo `last_tx_id` do payload de pareamento — só a chave e o `owner_address` viajam; a versão real só vem de um `pull` de verdade. **34 testes** (unitários por módulo + `tests/engine_lifecycle.rs`, round-trip completo simulando dois devices via telefone/gateway/par de pareamento falsos, mesmo padrão do `fake_phone.rs` do `warden-truthid`) — `cargo test -p warden-sync` e `cargo clippy --workspace` limpos. **Nunca testado contra o TruthID/Arweave reais** — mesma lacuna aceita de sempre (P38), registrada de novo em `PENDING.md`. Integração desktop: `desktop/src-tauri/src/sync_cmds.rs` (7 comandos Tauri, QR renderizado como SVG via `qrcode` crate), tela `SyncView.tsx` nova na sidebar. Integração CLI: `/sync`, `/sync push` (QR em Unicode direto no terminal — funciona por SSH numa máquina sem tela), `/sync pull`, `/sync pair`/`/sync pair <code>` |
| Fase 4.4 — vault local + sync pleno no mobile (P53) | Expor `warden-sync` direto via FFI vs uma casca fina; Gradle/Xcode escritos à mão vs uma ferramenta de build dedicada; onde resolver os paths do vault no mobile | **Novo crate `crates/warden-mobile-bridge`** ✓ (Sessão 55) — primeira ponte Rust↔Flutter do projeto. Casca fina (não expõe `warden-sync` direto): funções `#[frb]` (`bridge_status`/`bridge_init_fresh`/`bridge_push_begin`+`bridge_push_await`/`bridge_pull`/`bridge_pairing_host_start`+`bridge_pairing_host_wait`/`bridge_pairing_join`) instanciando um `warden_sync::SyncEngine` por chamada a partir de 4 paths (`String`) que o Dart resolve via `path_provider`'s `getApplicationSupportDirectory()` — mobile não tem `dirs::config_dir()` confiável em Android/iOS, então esses paths não podem vir dos defaults de `warden_sync::paths` (esses continuam servindo só desktop/CLI). `config.toml` sincroniza como blob opaco — o mobile nunca lê nem escreve nele (é cliente puro do `warden-server`, sem providers/agentes próprios), `SyncEngine`/`diff::config_changed` já toleram o arquivo não existir. **Toolchain de build: `cargokit`** (`flutter_rust_bridge_codegen integrate --integration-backend cargokit`), não Gradle/Xcode à mão — resolve NDK e as 4 ABIs Android automaticamente a partir do `Cargo.toml` do crate apontado (`cargo-ndk` como única ferramenta nova instalada; o toolchain Android em si, `~/.local/opt/android-sdk/ndk` 27/28, já existia de sessões anteriores e foi só reaproveitado). **Runtime async**: `SyncEngine` é construído sobre Tokio (`reqwest`, `tokio-tungstenite`), que precisa de um reactor rodando — `flutter_rust_bridge` já despacha qualquer `pub fn` não-anotada `#[frb(sync)]` pra uma thread de fundo própria por padrão (confirmado lendo o `init_app` gerado pelo template), então cada função do bridge só chama `.block_on()` num `tokio::runtime::Runtime` próprio (`OnceLock`, criado uma vez) sem nunca travar a UI do Dart — mais simples que tentar casar o executor do FRB com o de Tokio. **Override manual de host no pareamento** — decisão de produto, não só de teste: `pairing_join_with_hosts` (já existia no `SyncEngine`, só usado em teste antes) fica acessível por um campo opcional na tela de Sync do mobile, porque o sweep automático de LAN não atravessa a fronteira NAT de um emulador Android (rede virtual `10.0.2.x`) nem qualquer wifi com isolamento de cliente — mesma classe de problema que a 7.2/7.3 já resolveram com `10.0.2.2` explícito no lado da conexão ao `warden-server`. **Verificado de ponta a ponta contra hardware real (emulador Android), zero mock**: `.so` compilado pras 4 ABIs via `cargo-ndk`/cargokit, instalado e aberto via `adb` no `warden_test`, pareamento real contra um segundo processo `SyncEngine` isolado (fora do container do emulador, path de teste — nunca tocou `~/.config/warden/` real), `vault_key` idêntico confirmado nos dois lados via `adb run-as`, e `pending_vault_changes` refletindo um arquivo escrito de verdade no vault local do emulador — prova que o motor de diff/hash roda mesmo dentro do binário compilado pra Android, não só a camada de rede. Push/pull reais contra Arweave/TruthID e o lado iOS (scaffold do cargokit/FRB no lugar, nunca buildado — sem Xcode/macOS) seguem sem teste, mesmas lacunas já aceitas em P38/P55 e P39/P44. Ver `PENDING.md` P53 pro resto dos detalhes |
| Fase 4.5 — busca semântica no vault (P6) | Embedding local (ONNX) vs via API (OpenAI/Gemini) | **Local, via ONNX (`fastembed`, modelo `AllMiniLML6V2`)** ✓ (Sessão 56) — decisão explícita do usuário, com trade-offs postos antes de codar: local mantém a busca offline, não depende de qual provider de modelo o usuário configurou (Anthropic nem tem endpoint de embedding), sem custo por busca, e sem mandar o vault pra fora só pra indexar (mesmo conteúdo que já sai quando o LLM é chamado de verdade, não uma superfície nova). Custo aceito: primeira chamada de embedding baixa o modelo (~dezenas de MB) via `hf_hub` — exige rede uma vez por máquina, cacheado em `dirs::cache_dir()/warden/models` (não é conteúdo do vault, não duplica por vault). **Onde o índice vive**: `<vault_root>/.warden/semantic_index.json` — dentro do próprio vault, num diretório oculto, não em `~/.config/warden/` ao lado de `sync_manifest.json`. Motivo: `Vault::new` só recebe `root: PathBuf` hoje (dezenas de call sites em binários diferentes); um `.warden/` dot-prefixed é automaticamente ignorado tanto por `Vault::list_files`/`list_all_files` (`is_dotfile` já pula qualquer entrada, arquivo ou diretório, começando com `.`) quanto pelo sync (que usa exatamente `list_all_files`) — zero mudança em `warden-sync`, e cada vault de teste ganha seu índice isolado de graça. **Staleness self-healing, não hooks de escrita**: em vez de interceptar `WriteFileTool`/`bundle::apply_bundle` do sync (dois choke points em crates diferentes, e nenhum cobriria edição do vault por fora do Warden — o vault é Obsidian-compatible, editável por qualquer editor), `Vault::search_semantic` recalcula hash sha256 por chunk a cada chamada e só reembeda o que mudou — o mesmo custo de "ler todo arquivo toda vez" que o grep já pagava, mais embedding só do delta. Chunking é janela fixa de 40 linhas (não paragraph-aware) — "bom o bastante" pra notas curtas, documentado como limitação aceita. **Resiliência sem novo toggle de config**: `Orchestrator::handle_turn_streaming` tenta `search_semantic` (dentro de `tokio::task::spawn_blocking`, primeiro uso desse padrão no projeto — inferência ONNX é síncrona e pesada, `grep` original nunca precisou disso) e cai pro `search` (grep, intocado) em qualquer erro — sem precisar de um `[vault_search] engine = ...` em `config.toml` nem tela nova de Settings. `Vault` ganhou `embedder: Mutex<Option<TextEmbedding>>` (lazy, carrega o modelo uma vez por processo, reaproveitado enquanto o `Arc<Vault>` viver) e `index_lock: Mutex<()>` (protege a seção crítica de ler-atualizar-persistir o índice contra dois canais batendo no mesmo vault ao mesmo tempo via `warden-server`). **Verificado de ponta a ponta com o modelo real baixado de verdade** (rede disponível neste ambiente, ao contrário da maioria das outras pendências de "sem infra externa" do projeto): ranking correto distinguindo "consulta médica" de "compromisso com dentista" sem nenhuma palavra em comum, e comportamento incremental (edição do arquivo refletida na próxima busca) confirmado — testes em `crates/warden-core/tests/semantic_search.rs`, `#[ignore]`d por padrão (não hermético — baixa da Hugging Face) pra `cargo test`/CI ficarem rápidos independente de rede disponível, mesma postura de outras verificações reais deste projeto. **Achado no meio do caminho, não relacionado à decisão em si**: `cargo test --workspace` estourou o disco (`/home` só com 263MB livres, `Bus error` do linker ao compilar o binário de teste do `desktop`) — a árvore de dependências do `fastembed` (`ort`/`tokenizers`/`image`) é pesada o bastante pra empurrar um `target/` que já estava grande (43GB) além do limite; resolvido com `cargo clean` (48GB liberados), sem relação com a lógica do código |
| Fase 7.5 — notificações push no mobile | Push de verdade (FCM/APNs) vs notificação local (app vivo em background) | **Notificação local via `flutter_local_notifications`** ✓ (Sessão 56) — decisão explícita do usuário, com trade-offs postos antes de planejar: evita a primeira dependência de nuvem de terceiro do projeto inteiro (nada de projeto Firebase/conta Google, nada de certificado Apple), consistente com a filosofia self-hosted/local-first já estabelecida (mesmo espírito da decisão de embedding local na 4.5). Trade-off aceito: só funciona enquanto o processo do app estiver vivo — não sobrevive o app sendo `swipe-killed` da lista de recentes (mas **sobrevive só estar em background**, ver verificação abaixo — não é o mesmo que "processo morto"). **Gatilho**: `_ChatScreenState` (`mobile/lib/screens/chat_screen.dart`) passou a usar `WidgetsBindingObserver`/`AppLifecycleState` — como não existe hoje nenhum jeito de navegar pra fora do chat sem desconectar (P41), "app em foreground" e "olhando pro chat" são a mesma coisa, então `state != AppLifecycleState.resumed` já resolve "devo notificar?" sem precisar de um observador de rota. Módulo novo `mobile/lib/services/chat_notifications.dart`: `shouldNotifyFor`/`notificationContentFor` são funções puras testadas sem platform channel (mesma separação pure-vs-plugin já usada no `warden-cli` — `wrap_spans`/`card_width` testados sem terminal real), `initializeChatNotifications`/`requestNotificationPermission`/`showChatNotification` são wrappers finos sobre o plugin. Uma notificação só (`id` fixo), não uma por mensagem — substitui em vez de empilhar. Sem toggle de config novo — sempre ativo quando conectado e em background, não pedido pelo usuário. **Achado real no meio do caminho, sem relação com a decisão em si**: `fastembed` (Fase 4.5, sessão anterior) quebrava a compilação cruzada do `warden-mobile-bridge` pra Android — dois problemas empilhados: (1) `ort` (runtime ONNX do fastembed) não tem binário pré-compilado pra `armv7-linux-androideabi`; (2) os defaults do `fastembed` puxam `native-tls`/`openssl-sys`, que não cross-compila sem uma build de OpenSSL pro alvo. Isso nunca tinha aparecido porque a 4.5 só foi testada em builds de host (x86_64) — essa sessão foi a primeira tentativa de build Android desde então. Corrigido tornando `semantic-search` uma feature opcional em `warden-core` (`default = ["semantic-search"]`, `fastembed`/`sha2`/`dirs` todos `optional = true`), desligada explicitamente só em `warden-sync` (`warden-core = { path = ..., default-features = false }` — esse crate nunca usa `Orchestrator`/chat, só `Vault` para I/O de arquivo), e o `fastembed` em si trocado pra `default-features = false` + features `*-rustls-tls` (consistente com o resto do workspace, que já padroniza em rustls). `Orchestrator::handle_turn_streaming` ganhou dois braços `#[cfg(feature = "semantic-search")]`/`#[cfg(not(...))]` pro call site da busca semântica, já que `warden-core` precisa compilar sozinho sem a feature também. Também achado nesta sessão: `WidgetsFlutterBinding.ensureInitialized()` faltava em `main.dart` — inofensivo enquanto só `RustLib.init()` (FFI puro) rodava antes do `runApp`, mas `flutter_local_notifications` fala por `MethodChannel`, que exige o binary messenger pronto antes de qualquer chamada; sem isso o app crashava na inicialização com uma tela em branco. **Verificado de ponta a ponta contra hardware real (emulador Android), zero mock, nenhum passo pulado**: APK debug reconstruído com as duas correções, instalado via `adb`; prompt de permissão de notificação real (Android 13+) aceito e confirmado via `adb shell dumpsys package`; mensagem mandada a um `warden-server` real (`--config` com uma chave OpenAI inválida de propósito, escolhida deliberadamente pra ter uma resposta rápida e determinística sem precisar de uma chave real nem esperar um modelo responder de verdade) através de um app real conectado via `10.0.2.2`; app levado pro background (`adb shell input keyevent KEYCODE_HOME`) antes da resposta chegar; notificação real do Android confirmada na bandeja (`dumpsys notification` mostrando o `NotificationRecord` de verdade, canal `chat_messages`, e uma screenshot da bandeja puxada via `adb shell cmd statusbar expand-notifications` mostrando título "warden-server — Error" e o corpo do erro) enquanto o processo seguia **não congelado** (`dumpsys activity processes` confirmando `isFrozen=false` o tempo todo); toque na notificação reabriu o app de volta na `ChatScreen` com a conversa intacta. As primeiras tentativas de reproduzir esse fluxo via `adb shell input tap` erraram as coordenadas do botão de enviar (a barra de input muda de posição na tela dependendo do teclado estar aberto ou não) — resolvido lendo `uiautomator dump` pra pegar as coordenadas reais em vez de estimar pela screenshot, uma lição de metodologia registrada aqui pra sessões futuras que precisem automatizar UI do Flutter via `adb` |
| Um agente por conversa (P45) — trava a nível de UI vs enforcement no backend | O pedido é "não deixar trocar de agente no meio da conversa" — dava pra impor isso validando no lado Rust (rejeitar `send_message` se o `agent_id` da chamada divergir do já gravado na `Conversation`) ou só remover o affordance de troca da UI, sem validação nova no backend | **Só UI, sem validação no backend** ✓ (Sessão 57) — `Conversation.agent_id` (Sessão 43) já é o campo certo, por-conversa, só faltava parar de oferecer troca; um enforcement no lado Rust seria útil contra um cliente adversarial ou um segundo canal escrevendo na mesma conversa, nenhum dos dois é o caso aqui (desktop é single-user, single-writer). `send_message` continua recebendo `agent_id` em toda chamada exatamente como antes (Sessão 43) — o que muda é só o `ChatArea.tsx` nunca mais oferecer um valor diferente do já gravado depois da primeira mensagem |
| Um agente por conversa (P45) — onde encaixar o passo de escolha, sem estado novo | Precisava de um jeito de saber "conversa nova, agente ainda não escolhido" (mostra o picker, bloqueia o composer) vs "conversa nova, agente já escolhido" (composer liberado, ainda sem mensagens) vs "conversa em andamento" (agente travado) | **Reaproveita estado que já existia** ✓ — `ChatArea.tsx` já computava `hasMessages` (uma `Conversation` só é persistida a partir da primeira mensagem, `appendMessage` em `App.tsx`) e `selectedAgentId` já resetava pra `""` ao trocar pra uma conversa sem agente salvo (`useEffect` da Sessão 43). `needsAgentPick = !hasMessages && !selectedAgentId` cobre o primeiro caso sem nenhum estado novo em `App.tsx` — só `ChatArea.tsx` ganhou lógica de renderização condicional a mais (o `<select aria-label="Agent">` virou um rótulo somente-leitura fora desse caso, e um `.agent-picker` novo — cards clicáveis por `AgentEntry`, ou direcionamento pra Settings se `agents.length === 0` — dentro dele) |
| Um agente por conversa (P45) — escopo confirmado com o usuário antes de planejar | Aplicar a trava só no desktop (onde o seletor mid-conversa existe) vs também no CLI (`/agents use`, mesmo padrão de troca livre) vs em todos os canais (Telegram/WhatsApp/mobile não têm conceito de agente nenhum hoje, seria construir do zero) | **Só desktop** ✓ (Sessão 57) — a formulação original da P45 era especificamente sobre o `chat-header`; estender pro CLI ou pros canais sem agente ficou fora desta rodada, registrado como possível trabalho futuro se o usuário pedir. Também confirmado: agente passou a ser **obrigatório** (não existe mais opção "sem agente" numa conversa nova — zero agentes cadastrados direciona pra Settings em vez de deixar seguir sem persona) e o seletor de **modelo/provider não é afetado**, continua livre pra trocar a qualquer momento |
| Estrutura fixa do vault (P52, parte 1) — always-on vs search-triggered | O vault hoje só entra no prompt via `search`/`search_semantic`, ranqueado por relevância ao turno atual, com corte de 8 hits — sem nenhuma garantia de que um arquivo específico apareça. O usuário quer uma parte fixa (perfil, comportamento, feedback) sempre presente, não dependente de busca | **Novo caminho independente, `Vault::standing_memory()`** ✓ (Sessão 57) — lê os 3 arquivos fixos (`self.read`, síncrono, arquivos pequenos, sem precisar do `spawn_blocking` que a busca semântica usa por causa da inferência ONNX) e monta um bloco único, ignorando seção vazia/ausente. Injetado em `Orchestrator::handle_turn_streaming` como uma mensagem de sistema própria, **entre** a persona e o bloco de busca por relevância — ordem final: persona → memória fixa → busca (se houver hit) → histórico → turno atual. Como o hook fica no único ponto real de implementação (`handle_message`/`handle_turn`/`handle_message_streaming`/`handle_message_with_attachments` são todos wrappers finos dele), todos os canais herdam de graça, sem tocar em nenhum deles individualmente |
| Estrutura fixa do vault (P52, parte 1) — onde/como reservar os 3 arquivos | Nome/local dos arquivos fixos (perfil, comportamento, feedback): raiz do vault vs subpasta dedicada; dot-prefixed (como `.warden/`, que já é ignorado por tudo) vs visível | **Raiz do vault, prefixo `_`** ✓ (Sessão 57) — decisão explícita do usuário: `_profile.md`/`_behavior.md`/`_feedback.md`, visíveis (não dot-prefixed) porque precisam continuar sincronizando via `warden-sync` (que só ignora dotfiles) e aparecendo pro usuário num editor tipo Obsidian — diferente do `.warden/semantic_index.json`, que é estado interno, não conteúdo do usuário. `FIXED_VAULT_FILES` (const em `crates/warden-core/src/memory/mod.rs`) é a fonte única do nome dos 3, reaproveitada tanto pelo seeding (`warden-bootstrap`) quanto pela exclusão da busca (abaixo) |
| Estrutura fixa do vault (P52, parte 1) — os 3 arquivos entram na busca (grep/semântica) normal? | Deixar `search`/`search_semantic` tratar os arquivos fixos como qualquer nota (podem aparecer como "hit") vs excluí-los | **Excluídos** ✓ (Sessão 57) — `collect_markdown_files` (usada por `list_files`, logo por `search`/`search_semantic`) pula os 3 nomes reservados **só quando estão na raiz do vault** (`is_fixed_vault_file`, checa `path.parent() == root`) — um arquivo do usuário chamado `notes/_profile.md`, embora improvável, continua pesquisável normalmente, só o da raiz é especial. Motivo: sem isso, o mesmo conteúdo apareceria duas vezes no prompt (uma vez fixo, outra como "hit" de busca) e ainda consumiria vaga do orçamento de 8 hits em prejuízo das notas livres de verdade. `list_all_files` (sync) não é tocada — os 3 continuam sincronizando normalmente |
| Estrutura fixa do vault (P52, parte 1) — seeding: quando e como, sem sobrescrever um vault já existente | Onde criar o template inicial dos 3 arquivos na primeira vez, sem clobber um vault clonado/restaurado via `warden-sync` de outro device (que já pode ter esses arquivos, possivelmente editados) | **`seed_default_vault_files`, logo após `Vault::new` em `bootstrap()`** ✓ (Sessão 57) — checagem por arquivo (`vault.read(name).is_err()`), só escreve o template se o arquivo ainda não existe; nunca sobrescreve. Um único ponto de chamada (`bootstrap()`, `crates/warden-bootstrap/src/lib.rs`) cobre todos os canais, já que todos passam por ele pra construir o `Orchestrator`. Templates curtos, em português (mesma língua que o usuário já usa pro conteúdo do próprio vault, diferente das strings de UI do app, que são em inglês) — título + uma linha de orientação tanto pro usuário quanto pro modelo saberem o que vai ali, incluindo uma instrução explícita em `_feedback.md` pra IA atualizar o arquivo sozinha quando aprender algo relevante (via a `WriteFileTool` já existente, sem tool nova) |
| Estrutura fixa do vault (P52, parte 1) — cap de tamanho do bloco sempre injetado | Por ser injetado em toda chamada (não dependente de busca), o custo em tokens cresce com o que o usuário/IA escrever nos 3 arquivos — sem limite, um arquivo enorme entra inteiro em toda mensagem | **Sem cap nesta rodada** ✓ (Sessão 57) — trade-off aceito e documentado no código (doc-comment de `standing_memory`), mesmo espírito de "shipar simples primeiro" já usado no projeto (ex. chunking fixo de 40 linhas da 4.5, sem paragraph-aware). Se algum dia incomodar, um teto de caracteres é a extensão natural, não implementado agora |
| Visualização do vault (P52, parte 2) — leitura vs. edição | UI nova no desktop pra navegar o vault: deixar editar direto na tela (economiza abrir outro editor) vs. só mostrar o conteúdo renderizado | **Só leitura** ✓ (Sessão 57) — decisão explícita do usuário: edição continua por fora (Obsidian/qualquer editor de texto — o vault é uma pasta comum) ou pela própria IA via `WriteFileTool` (já existe, sem tool nova). Editor completo (salvar, undo, conflito se a IA escrever no mesmo arquivo ao mesmo tempo) é escopo bem maior, fica pra uma v2 se fizer falta |
| Visualização do vault (P52, parte 2) — onde os comandos IPC vivem | `lib.rs` já tinha 552 linhas antes desta mudança — comandos novos (`list_vault_files`/`read_vault_file`) direto nele vs. módulo próprio | **`vault_cmds.rs` novo** ✓ (Sessão 57) — mesmo precedente já estabelecido por `sync_cmds.rs` (que documenta explicitamente por que foi separado: cluster de comandos autocontido, `AppState` continua acessível porque a visibilidade de um item privado em Rust se estende a todo módulo descendente do que o declara, não só o módulo exato). `mod vault_cmds;` no topo de `lib.rs`, comandos registrados como `vault_cmds::list_vault_files`/`vault_cmds::read_vault_file` no mesmo `generate_handler!`, igual `sync_cmds::*` |
| Visualização do vault (P52, parte 2) — de onde vem o `Vault` usado pelos comandos novos | Reconstruir um `Vault::new(vault_path)` do zero a cada chamada IPC (re-lendo `config.toml` toda vez, como `get_settings`/`save_settings` já fazem) vs. reaproveitar um handle já vivo | **Reaproveita `AppState.orchestrator`** ✓ (Sessão 57) — `Orchestrator::vault()` (`&Arc<Vault>`) já existe; `state.orchestrator.lock().unwrap().clone()` é o mesmo padrão que `send_message` já usa pra pegar um `Orchestrator` (barato, tudo por trás é `Arc`). Erro de bootstrap anterior (`Result::Err(String)` guardado no `Mutex`) se propaga pelo `?` do mesmo jeito que já acontece em `send_message`, sem tratamento novo |
| Visualização do vault (P52, parte 2) — os 3 arquivos fixos aparecem destacados ou junto com o resto | Seção própria no topo da navegação vs. árvore de pastas comum, sem tratamento especial (o `_` já os ordena primeiro na maioria dos casos) | **Seção própria, destacada** ✓ (Sessão 57) — decisão explícita do usuário, reflete a distinção real que a parte 1 criou (esses 3 são *sempre* injetados no prompt, o resto só por busca). Rótulos hardcoded no frontend (`VaultView.tsx`, `FIXED_FILES`) em vez de buscados por IPC — mesma ordem/nomes de `FIXED_VAULT_FILES`/`standing_memory` em `warden-core`, mas um contrato estável e pequeno o bastante (3 nomes fixos, dificilmente muda) pra não justificar um comando novo só pra listar isso. Labels em português (mesma língua do template real de cada arquivo, cujo próprio `# ` inicial já é em português) — chrome ao redor (título "Vault", textos de estado) fica em inglês, mesma convenção já usada no resto do desktop (UI em inglês, conteúdo do usuário/persona no idioma que ele escreveu) |
| Visualização do vault (P52, parte 2) — renderização de markdown | Nenhuma lib de markdown existia fora de `MessageBubble.tsx` (bolhas de chat) — trazer uma nova pra `VaultView` vs. reaproveitar | **Reaproveita `react-markdown`/`remark-gfm`**, já dependência do projeto ✓ (Sessão 57) — zero dependência nova. `MarkdownLink` (intercepta clique de link, abre no navegador padrão via `@tauri-apps/plugin-opener` em vez de navegar o webview) só precisou virar `export` em vez de ficar privado ao arquivo |
| Visualização do vault (P52, parte 2) — arquivo fixo vazio/nunca escrito vs. erro real de leitura | `read_vault_file` retorna `Err` tanto pra um arquivo fixo que o usuário nunca preencheu quanto pra um erro de verdade (ex. arquivo não-UTF-8) — tratar os dois igual (mostrar erro) ou diferenciar | **Diferenciado no frontend** ✓ (Sessão 57) — `VaultView` sabe se o `selectedPath` é um dos 3 fixos (`FIXED_FILES.some(...)`); se for e a leitura falhar, mostra um placeholder discreto ("Empty — nothing written here yet"); se for qualquer outro arquivo da árvore, mostra o erro de verdade (`usage-error`), isolado ao painel de conteúdo — nunca derruba a navegação nem o resto da tela |
| Autenticação servidor↔cliente | Chave local vs TruthID | **Chave local no v1** ✓ — TruthID pode vir depois |
| Extensão de navegador | Canal + Tool provider vs só canal | **Canal + Tool provider** ✓ — expõe DOM/clique/navegação como tool MCP |
| Framework de CLI | clap vs structopt vs gum | **clap v4 (derive)** ✓ — structopt foi descontinuado e incorporado ao clap desde a v3; gum é ferramenta Bash/TUI, não se aplica a Rust |
| API de `web_search` | Tavily vs Brave Search API vs Google Custom Search | **Tavily** ✓ — feita pra tool use de agentes LLM (resultados já vêm resumidos), free tier de 1.000 buscas/mês sem cartão de crédito. Tool só é registrada se `TAVILY_API_KEY` estiver setada (degradação graciosa, mesmo espírito de P14) |
| Formato do arquivo de config | TOML vs YAML | **TOML** ✓ — convenção do próprio ecossistema Rust (mesmo formato do `Cargo.toml`), sem as ambiguidades clássicas de parsing do YAML, crate `toml` madura e serde-native. Localização default via crate `dirs` (`~/.config/warden/config.toml` no Linux, equivalente no Windows/macOS), com override via `--config`. Precedência: flag de CLI > variável de ambiente (só pra API keys) > arquivo de config > default embutido |
| Lógica de bootstrap compartilhada entre canais | Duplicar em cada canal vs crate à parte vs meter em `warden-core` | **Novo crate `crates/warden-bootstrap`** ✓ — carrega o config TOML e monta o `Orchestrator` (provider/vault/tools/sub-agente) uma vez só, usado por `warden-cli` e por `desktop/src-tauri`. Não entrou em `warden-core` de propósito (fonte de config é decisão de camada de aplicação/canal, não do motor model-agnostic); não virou "warden-cli como lib" pra não misturar parsing de CLI (clap) com algo que um app GUI também precisa. `default_vault_path` é parâmetro da função `bootstrap(...)` — cada canal informa o fallback certo pro seu contexto (CLI: `"vault"` relativo ao cwd; desktop: `~/Warden/vault`, já que o cwd de um app lançado por ícone é imprevisível) |
| Formato/local do histórico de conversas (6.6) | JSON no config dir vs dentro do vault markdown vs banco embutido (sqlite) | **JSON, um arquivo por conversa, em `~/.config/warden/conversations/<id>.json`** ✓ — mesmo `dirs::config_dir()` já usado pro `config.toml`, por ser dado opaco de app (não é conhecimento humano-navegável tipo o vault, que fica de propósito fora dessa pasta). Um arquivo por conversa (não um índice único) evita reescrever tudo a cada mensagem e isola corrupção — `list_conversations` já pula arquivo malformado em vez de falhar a lista inteira. Lógica em `warden-bootstrap` (`Conversation`/`ConversationMessage`/`save_conversation`/`list_conversations`), mesmo padrão de `save_config`/`load_config_from_path`; comandos Tauri (`list_conversations`/`save_conversation`) só chamam essas funções |
| Build/release do app desktop (6.8) | GitHub Actions (matrix por OS) vs cross-compilação local vs serviço de build de terceiro | **GitHub Actions, `.github/workflows/build.yml`** ✓ — replica quase literalmente o `build.yml` do TruthID (mesmo stack Tauri v2): dispara em tag `v*`, matrix `ubuntu-22.04`/`windows-latest`/`macos-latest`, `tauri-apps/tauri-action@v0` builda e publica um GitHub Release **draft** por tag. Sem assinatura de código nem notarização macOS (mesma decisão já tomada no TruthID — não bloqueante pro v1). Cross-compilação local nunca foi opção real: não tem SDK da Apple nem toolchain Windows nesta máquina de dev (Arch Linux) |
| Segurança da tool `shell` (5.5) | Sempre ativa (mesmo padrão de `read_file`/`write_file`) vs opt-in vs allowlist/sandboxing | **Opt-in, desligada por padrão** ✓ — decisão explícita do usuário, não da IA: uma tool de shell é categoricamente mais arriscada que as tools de arquivo (execução arbitrária de comando vs leitura/escrita já sem scoping real). Gate via `resolve_flag(WARDEN_ENABLE_SHELL, config.enable_shell)` (mesma precedência env > config do `resolve_secret`, mas pra booleano), com toggle também na tela de Settings do desktop (`enable_shell` em `SettingsSnapshot`/`SettingsFormPayload`) — religar/desligar já dá live-reload do orchestrator de graça, reaproveitando o mecanismo da 6.5. **Nenhum sandboxing/allowlist além disso** — uma vez ligada, o modelo pode rodar qualquer comando; as únicas proteções são `timeout_ms` (default 30s, capado em 5min, mata o processo via `kill_on_drop`) e truncamento de stdout/stderr (~20KB) pra não estourar o contexto. Execução via `sh -c` (Unix) / `cmd /C` (Windows) — PowerShell fica de fora por ora. `cwd` default = raiz do vault, overridável (relativo ou absoluto) |
| Registry de tools dinâmico (5.1) | Continuar só com `Vec<Arc<dyn Tool>>` fixo no `Orchestrator` vs um novo conceito de "fonte de tools descoberta em runtime" | **Trait `ToolProvider`** (`warden_core::tool::ToolProvider`, `async fn tools(&self) -> anyhow::Result<Vec<Arc<dyn Tool>>>`) ✓ — motivado pelo MCP client (5.2): conectar num server MCP não dá uma tool fixa, dá um conjunto de N tools só conhecido depois do handshake (`tools/list`). `Orchestrator::register_provider` chama `tools()` e registra cada uma via `register_tool` — é um snapshot no momento da chamada, sem live-sync se o server mudar seu conjunto de tools depois |
| MCP client (5.2) — SDK | Implementar o handshake JSON-RPC do MCP na mão vs usar SDK | **`rmcp`** ✓ (crates.io, mantido pela org `modelcontextprotocol`) — cobre `initialize`/`tools/list`/`tools/call` prontos; reimplementar seria retrabalho sem ganho, já que é um protocolo padronizado. Features habilitadas: `client` + `transport-child-process` em `warden-core` (produção); `transport-io` só em `[dev-dependencies]`, usado exclusivamente pelo teste real de subprocess (`tests/mcp_stdio.rs`) |
| MCP client (5.2) — transporte | stdio (child process) vs HTTP remoto | **stdio** ✓ para v1 — é o padrão de facto pra MCP local (mesmo shape `command`/`args`/`env` de qualquer client MCP, ex. Claude Desktop), cobre o caso concreto do usuário (Anchor/TruthID rodando local — P11/P13). `McpToolProvider::connect_stdio(name, command, args, env)` em `warden-core/src/tool/mcp.rs`; `env` é escopado só pro processo filho (`Command::envs`), nunca muta o processo do Warden. HTTP remoto fica pra quando surgir um caso real — `rmcp` já suporta, a interface (`ToolProvider`) não é stdio-specific |
| Config de servers MCP | Onde/como o usuário configura quais servers conectar | **`[[mcp_servers]]` no `config.toml`** ✓ (`FileConfig.mcp_servers: Vec<McpServerConfig>`, cada um com `name`/`command`/`args`/`env`) — mesmo formato que outros clients MCP usam, pra portar config existente quase verbatim. `bootstrap()` conecta em cada um e registra as tools; falha em conectar/listar não derruba o app, só loga um aviso e pula aquele server (mesmo espírito de degradação graciosa do Tavily/shell). **Sem UI ainda** — só editável a mão no config file; a tela de gerenciamento (adicionar/remover visualmente) é P11 em `PENDING.md`, ainda em aberto. Efeito colateral: `bootstrap()` virou `async fn` (precisa spawnar processo + aguardar handshake), o que exigiu `tauri::async_runtime::block_on` no `run()` do desktop (chamado antes do runtime do Tauri iniciar) e `save_settings` virar comando Tauri assíncrono |
| Tool `web_search` (5.3) | Manter REST hand-rolled (Tavily API direto, Fase 1.7) vs substituir pelo server MCP oficial da própria Tavily (`tavily-mcp`) | **Substituído por `tavily-mcp`** ✓ — decisão explícita do usuário (trade-off real, não óbvio): a versão REST era Rust puro, só exigia a API key; a versão MCP roda via `npx -y tavily-mcp`, então passa a exigir Node.js/`npx` no PATH em runtime pra essa capacidade. Aceito porque (a) a Tavily mantém o próprio server MCP oficial (https://docs.tavily.com/documentation/mcp), então não é uma integração de terceiro frágil; (b) dá busca **e** extract/crawl/map/research de graça (5 tools, confirmado rodando contra o server real, não só o de teste da 5.2); (c) serve de validação de ponta a ponta do client MCP contra um server publicado de verdade. `WebSearchTool` (`tool/web_search.rs`) foi removido — o gate por `TAVILY_API_KEY` continua igual (`resolve_secret`), só que agora chama o novo helper `register_mcp_server_tools` em vez de construir a tool direto. Esse helper também passou a ser reaproveitado pelo loop de `config.mcp_servers`, que tinha exatamente o mesmo padrão connect→list→extend duplicado |
| Tool `file_system` (5.6) — escopo | Substituir `read_file`/`write_file` (vault) vs capacidade nova e separada, fora do vault | **Capacidade nova e separada** ✓ — decisão explícita do usuário. `read_file`/`write_file` continuam só pro vault (uso interno de memória); `file_system` é acesso a diretórios arbitrários do sistema, no espírito do `shell` (5.5) mas pra I/O de arquivo em vez de execução de comando. Motivado em parte por uma lacuna real encontrada em `memory/mod.rs`: `Vault::read`/`write` fazem só `root.join(path)`, sem canonicalizar nem conter — path traversal não é bloqueado hoje nos tools do vault |
| Tool `file_system` (5.6) — implementação | Campo de config dedicado (`file_system_allowed_dirs`) vs reusar o mecanismo genérico `[[mcp_servers]]` da 5.2 | **Reusar `[[mcp_servers]]`, sem campo novo** ✓ — diferente do Tavily (que ganhou tratamento dedicado por ter um "segredo" único e óbvio, `TAVILY_API_KEY`, já com campo na Settings UI), `file_system` não tem equivalente natural pra virar config especial: é só uma lista de diretórios arbitrários, exatamente o que `mcp_servers` já resolve de forma genérica. Um campo bespoke duplicaria o mecanismo sem adicionar capacidade — habilita-se via: `[[mcp_servers]] name = "file_system" command = "npx" args = ["-y", "@modelcontextprotocol/server-filesystem", "/caminho/permitido"]`. **Sem UI ainda** — mesma decisão do usuário já tomada pra `mcp_servers` em geral (P11 em `PENDING.md`). Verificado de ponta a ponta contra o server oficial de verdade (não mockado): 14 tools listadas (`read_file`, `read_text_file`, `write_file`, `edit_file`, `move_file`, `search_files`, etc.), round-trip real de escrita+leitura dentro do diretório permitido funcionou, e uma tentativa de escrever **fora** do diretório permitido foi rejeitada pelo próprio server (`"Access denied - path outside allowed directories"`, confirmado que o arquivo não foi criado em disco) — prova concreta de que essa capacidade é mais segura que o `read_file`/`write_file` atual do vault |
| Integração Google (5.7) — qual server MCP | O server oficial de referência (`@modelcontextprotocol/server-gdrive`) está **arquivado** (`servers-archived`, sem manutenção) — não existe mais opção oficial. Candidatos da comunidade pesquisados: `taylorwilsdon/google_workspace_mcp` (uvx/Python, 2987★, o mais completo — 120+ tools em 12 serviços) vs `aaronsb/google-workspace-mcp` (npx/Node.js, 164★, mantido ativamente, 11 tools em 7 serviços incl. multi-conta nativo) vs outros com pouca tração (`dguido` **arquivado**, `danielrosehill` 1★, `j3k0` 32★) | **`aaronsb/google-workspace-mcp` via `npx`** ✓ — decisão explícita do usuário: prevaleceu consistência de runtime sobre cobertura de tools. O projeto já aceitou Node.js/`npx` como dependência de runtime pro Tavily (5.3) e pro `file_system` (5.6) — ver P17 em `PENDING.md`; escolher o candidato uvx/Python teria introduzido um **segundo** runtime obrigatório (`uv`) só pra essa capacidade. `aaronsb` cobre exatamente o escopo do PHASE.md (Gmail/Calendar/Drive) mais Sheets/Docs/Tasks/Meet de bônus, com suporte a múltiplas contas Google já embutido (`manage_accounts`) |
| Integração Google (5.7) — implementação | Igual à 5.6: campo de config dedicado vs reusar `[[mcp_servers]]` | **Reusar `[[mcp_servers]]`, sem campo novo** ✓ — mesmo raciocínio da 5.6, credenciais são só duas env vars (`GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`), sem "segredo único e óbvio" que justifique um campo bespoke tipo o do Tavily. Habilita-se via: `[[mcp_servers]] name = "google_workspace" command = "npx" args = ["-y", "@aaronsb/google-workspace-mcp"] [mcp_servers.env] GOOGLE_CLIENT_ID = "..." GOOGLE_CLIENT_SECRET = "..."`. **Setup fora do Warden, feito pelo usuário**: criar um projeto no Google Cloud Console, habilitar as APIs desejadas (Gmail/Calendar/Drive/...), criar credencial OAuth 2.0 tipo "Desktop app" — não tem como automatizar isso do lado do Warden. No primeiro uso, a tool `manage_accounts` (operação `authenticate`) abre um browser pro consent OAuth; token fica salvo em `~/.local/share/google-workspace-mcp/credentials/`, por conta. Verificado de ponta a ponta contra o server real via `McpToolProvider::connect_stdio` (mesmo caminho de produção), **sem** credenciais setadas: conecta normalmente (server não exige env vars no handshake) e lista os 11 tools reais (`manage_email`, `manage_calendar`, `manage_drive`, `manage_docs`, `manage_sheets`, `manage_tasks`, `manage_meet`, `manage_accounts`, etc.) — confirma que a integração ponta a ponta funciona sem nenhum código novo; só falta o usuário configurar credenciais próprias pra usar de verdade |
| Rate limiting/custo por tool (5.8) — escopo | PHASE.md/P4 pediam "rate limiting e controle de custo por tool" — dois mecanismos distintos (limitar frequência vs rastrear/limitar gasto) | **Só tracking de uso** ✓ — decisão explícita do usuário, entre 4 opções apresentadas (tracking só; tracking + teto rígido de custo; rate limiting por tool; os dois). Captura e persiste os tokens que cada chamada de modelo já reporta (hoje 100% descartados — `grep` por `usage`/`cost`/`rate_limit`/`token` não batia em nada no repo inteiro), sem nenhum limite/bloqueio ainda. Base mínima pro dashboard de custo futuro (P10); rate limiting de verdade e teto de gasto configurável continuam em aberto, P4 estreitada pra cobrir só essa parte restante |
| Rate limiting/custo por tool (5.8) — implementação | Onde capturar e como propagar o uso até a persistência | **Novo tipo `Usage` (`warden_core::model`)** com `prompt_tokens`/`completion_tokens`/`total_tokens`, populado por cada provider a partir do que a própria API já devolve (`usage` da OpenAI, `usageMetadata` do Gemini — nomes de campo estáveis e documentados publicamente, não um mecanismo de terceiro a validar como na 5.3/5.6/5.7). `Response` ganha `usage: Option<Usage>`; `Orchestrator::handle_message` (único chokepoint por onde toda chamada de modelo passa) some o uso das até `MAX_TOOL_ITERATIONS` chamadas de uma mesma mensagem e devolve num novo tipo `MessageOutcome { content, usage }` no lugar do `String` que devolvia antes. `ConversationMessage` (bootstrap) ganha `#[serde(default)] usage: Option<Usage>` — `#[serde(default)]` é o que mantém conversas já salvas em disco (sem esse campo) carregando sem erro. Desktop: `send_message` passa a devolver `{content, usage}` em vez de `String` cru; frontend anexa um badge discreto de tokens só na mensagem do assistente (`MessageBubble.tsx`, `var(--color-text-muted)` + `0.82em`, mesma convenção já usada em outros textos secundários da UI — não havia nenhum metadado por mensagem renderizado antes disso). CLI: sem persistência de conversa nenhuma hoje, então é só um echo de visibilidade após a resposta, não storage. **Gap aceito e documentado no código** (`tool/delegate.rs`): o uso do sub-agente disparado por `DelegateTool` não sobe pro total da conversa pai — `Tool::call` só devolve `serde_json::Value`, não `MessageOutcome`; fechar isso exigiria mudar a trait `Tool` inteira, fora do escopo mínimo desta sessão |
| Canal Telegram (Fase 2) — trait `Channel` | `PHASE.md` (etapa 2.2) cita uma trait `Channel`, mas nem `warden-cli` nem o desktop compartilham hoje nenhuma abstração além de chamar `bootstrap()` uma vez cada — investigação não achou nenhuma trait `Channel` existente | **Sem trait ainda** ✓ — decisão explícita do usuário. Em vez disso, `warden-bootstrap` ganhou uma função concreta e reutilizável, `handle_turn(orchestrator, conversations_dir, conversation_id, title_seed, user_input) -> anyhow::Result<MessageOutcome>`: carrega a conversa por id (novo `load_conversation`, a versão "uma conversa só" do `list_conversations` existente), chama `orchestrator.handle_message`, acrescenta as duas mensagens novas e persiste. `crates/warden-telegram` usa isso; `warden-cli`/desktop continuam como estavam (CLI não persiste nada; desktop já faz seu próprio read/append/save no frontend). Motivo: uma trait com um único implementador não valida a forma certa da abstração — fica pra quando o WhatsApp (Fase 3) existir e dois exemplos reais mostrarem o que é de fato compartilhável (bot HTTP long-polling vs sidecar Node+IPC do Baileys são bem diferentes) |
| Canal Telegram (Fase 2) — cliente HTTP testável | Como testar o loop de receive→rotear→responder sem depender da API real do Telegram — o repo não tem nenhuma crate de mock HTTP (`wiremock` etc.) | **Trait fina `TelegramApi`** (`get_updates`/`send_message`) implementada de verdade por `TelegramClient` e mockada em teste por `ScriptedTelegramApi` — mesmo espírito do `ModelProvider`/`ScriptedModel` já usado em `warden-core/tests/pipeline.rs`, sem introduzir dependência nova só pra isso. `run_bot`/`process_updates`/`handle_update` (`crates/warden-telegram/src/telegram.rs`) são genéricos sobre `impl TelegramApi`, então os testes hermáticos rodam o loop de verdade (offset avança, `/start`/`/help` não chama o orchestrator, conversa persiste com as 2 mensagens) sem nenhuma chamada de rede |
| Canal Telegram (Fase 2) — formatação das respostas (etapa 2.5) | Texto puro vs MarkdownV2 real | **Texto puro (sem `parse_mode`)** ✓ — decisão explícita do usuário. O MarkdownV2 do Telegram tem sintaxe própria (diferente de CommonMark) e a API rejeita a mensagem inteira se um caractere reservado não escapado aparecer no texto — um conversor de verdade (CommonMark do modelo → MarkdownV2) é trabalho substancial, não só escapar caracteres. Fica pra uma sessão futura se formatação (negrito/listas/code block) fizer falta na prática |
| Canal Telegram (Fase 2) — diretório de conversas | Reusar `default_conversations_dir()` (o que o desktop lista no sidebar) vs um diretório próprio | **Diretório próprio**, `default_telegram_conversations_dir()` (`~/.config/warden/conversations-telegram/`) ✓ — decisão do agente, não perguntada (baixo risco, reversível): conversas do Telegram são tituladas por `chat_id`/username, sem client-side UI nenhuma, e misturá-las no mesmo diretório faria o sidebar do desktop listar entradas que ele não sabe rotular direito. Uma visão unificada de conversas entre canais é uma pergunta de produto maior, registrada como nota de `ROADMAP.md`, não resolvida aqui |
| Canal Telegram (Fase 2) — transporte | Long polling (`getUpdates`) vs webhook | **Long polling** ✓ — já antecipado no mapa de dependência de servidor deste arquivo ("Canal Telegram (long polling, Fase 2)"), consistente com o princípio geral do projeto de não exigir papel de servidor pra canais (P14 em `PENDING.md`): um processo sempre ligado no próprio device do usuário basta, sem precisar expor um endpoint HTTPS público que um webhook exigiria |
| Canal WhatsApp (Fase 3) — IPC sidecar↔core | `PHASE.md` deixava em aberto ("stdin/stdout ou socket"). Não existia nenhum precedente de socket/IPC customizado no repo — só o `TokioChildProcess` do client MCP (`crates/warden-core/src/tool/mcp.rs`), inteiramente específico do protocolo MCP (JSON-RPC via `rmcp`), não reaproveitável como transporte genérico | **stdin/stdout, JSON-lines** ✓ — decisão explícita do usuário. Reusa o mesmo padrão de spawn já usado pro MCP (`tokio::process::Command`), só com um protocolo JSON simples do próprio projeto em vez do MCP. Evita ser a primeira dependência de `tokio::net` do projeto, gerenciamento de arquivo de socket, e a diferença Unix-socket vs named-pipe do Windows. `crates/warden-whatsapp/src/sidecar.rs`: trait `WhatsAppSidecar` (`&mut self` — ao contrário da `TelegramApi` da Fase 2, que é `&self`, porque ler linha a linha de um stream é estado, não uma chamada nova a cada vez), `ChildSidecar` implementa de verdade sobre `ChildStdin`/`ChildStdout` via `tokio::io::{AsyncBufReadExt, AsyncWriteExt}` (nova feature `io-util` do tokio, só nesta crate); `ScriptedSidecar` mocka em teste, mesmo espírito da `ScriptedTelegramApi` |
| Múltiplos provedores de modelo (Sessão 35) — arquitetura | Enum fechado com um bra​ço por provedor (padrão até então: `Provider::Gemini`/`Provider::Openai`, 1 slot de API key cada em `FileConfig.api_keys`) vs um **registry**: lista de entradas configuráveis, cada uma independentemente selecionável | **Registry** ✓ — decisão explícita do usuário (ver P22 em `PENDING.md`), no mesmo espírito do `[[mcp_servers]]` já existente (5.2): `ProviderConfig { id, kind: Provider, api_key, base_url, model }` (`crates/warden-bootstrap/src/lib.rs`), `FileConfig` ganha `providers: Vec<ProviderConfig>` + `active_provider: Option<String>`. Escala pra qualquer provedor novo sem precisar de código novo pra cada um (só uma linha na UI, se nem isso — ver decisão seguinte sobre `OpenaiCompatible`) |
| Múltiplos provedores de modelo (Sessão 35) — quais provedores | Anthropic + Ollama dedicados (uma implementação própria pra cada) vs Anthropic dedicado + um tipo genérico "OpenAI-compatível" (`base_url` configurável) cobrindo Ollama e qualquer outro servidor que fale o mesmo protocolo (OpenRouter, Groq, DeepSeek, ...) | **Anthropic dedicado + `Provider::OpenaiCompatible` genérico** ✓ — decisão explícita do usuário. `AnthropicProvider` novo (`crates/warden-core/src/model/anthropic.rs`) implementa a Messages API própria da Anthropic (`system` como campo top-level em vez de mensagem, `tool_use`/`tool_result` como content blocks, `max_tokens` obrigatório — fixado em 4096, sem config própria ainda, mesmo escopo mínimo da 5.8). `OpenAiProvider` (`model/openai.rs`) ganhou `base_url` configurável (`with_base_url`, default continua `https://api.openai.com/v1`) — como o Ollama já expõe um endpoint OpenAI-compatível (`http://localhost:11434/v1`) e a maioria dos outros provedores "novos" (Groq, OpenRouter, DeepSeek, Together) também, isso cobre todos eles de graça sem precisar de uma implementação dedicada por empresa. `default_model_for` retorna `None` pra `OpenaiCompatible` (sem default universal — depende do que está hospedado), tornando `model` obrigatório nesse caso |
| Múltiplos provedores de modelo (Sessão 35) — compatibilidade com config existente | `FileConfig` tem `#[serde(deny_unknown_fields)]` — trocar os campos antigos (`provider`/`api_keys.gemini`/`api_keys.openai`) quebraria qualquer `config.toml` já escrito (inclusive o do próprio usuário, com uma chave Gemini real, criado na Sessão 32) | **Campos antigos mantidos no struct, só como fallback** ✓ — decisão do agente (baixo risco, reversível): `provider`/`model`/`api_keys.gemini`/`api_keys.openai` continuam existindo em `FileConfig` (documentados como deprecated), mas só são lidos por `resolve_model_provider` quando `providers` está vazio — nesse caso ele sintetiza uma única entrada a partir deles (incluindo os env vars `GEMINI_API_KEY`/`OPENAI_API_KEY`, exatamente como antes), preservando 100% do comportamento pra quem nunca tocou a Settings nova. Uma vez que o desktop salva pela tela nova, `save_settings` limpa esses campos de volta pra `None` (evita segredo duplicado obsoleto no arquivo). Não existe função de migração separada — a síntese acontece inline, a cada `bootstrap()`, o que também é o que mantém os testes de `warden-cli`/`warden-telegram`/`warden-whatsapp` (que só setam env vars, sem `config.toml` nenhum) passando sem nenhuma mudança |
| Múltiplos provedores de modelo (Sessão 35) — UI de gerenciamento no desktop | CRUD granular (comandos Tauri dedicados tipo `add_provider`/`delete_provider`) vs continuar com o padrão já existente de "form salva tudo de uma vez" (mesmo espírito do resto da Settings, que já carrega o config existente e reescreve o arquivo inteiro a cada Save) | **Form/lista editada localmente no React, salva de uma vez** ✓ — decisão do agente, consistente com o padrão já estabelecido (evita introduzir um novo estilo de IPC só pra isso). `SettingsView.tsx` ganhou uma seção "Model providers": cada entrada é um card com nome, tipo (select), campos condicionais (Base URL só aparece pra `openai_compatible`), API key (mascarada, toggle de revelar, reaproveitando o `ApiKeyField` já existente), Model (placeholder = default do backend via novo `defaultModels` em `SettingsSnapshot`), um rádio "Active" e um botão de apagar (remove da lista local — só persiste de fato no próximo Save, mesma semântica de "apagar" que o resto do formulário sempre teve). Validação de nome vazio/duplicado tanto no frontend (mensagem imediata) quanto no backend (`save_settings`, defesa em profundidade). Verificado de ponta a ponta via Playwright headless contra o dev server real (mesmo padrão da Sessão 22): adicionar 2 provedores, trocar tipo pra `openai_compatible` (Base URL aparece), marcar um como ativo, salvar — payload confirmado byte a byte no formato que o Rust espera, zero erros de console, apagar preserva o provedor ativo corretamente |
| MCP client — UI de gerenciamento de servers no desktop (P11a, Sessão 35) | `McpServerConfig` (`name`/`command`/`args`/`env`) já existia desde a 5.2, mas só editável a mão no `config.toml` | **Mesmo padrão da lista de provedores** ✓ — nova seção "MCP servers" na tela de Settings: cards com nome/comando/argumentos (textarea, um por linha — evita ambiguidade de quoting de um input space-separated)/variáveis de ambiente (linhas chave/valor dinâmicas, valor mascarado). `McpServerConfig` é reusado diretamente como tipo de IPC (sem um `*Payload` dedicado como o dos provedores) — seus campos já são todos de uma palavra só, não precisam de remapeamento pra `camelCase`. Validação de nome/comando vazio no backend (`save_settings`), mesma defesa em profundidade dos provedores. Uma vez salvo pela UI nova, o backend passa a usar o payload como fonte de verdade (parou de só "carregar e devolver" o `mcp_servers` existente, comportamento provisório de antes de existir UI) |
| MCP client — presets de "quick add" (P11a, Sessão 35) | Só oferecer um card em branco vs pré-preencher comando/args/env pra integrações populares conhecidas | **4 presets pesquisados e adicionados** ✓ — Filesystem (`@modelcontextprotocol/server-filesystem`, já documentado na 5.6), Google Workspace (`@aaronsb/google-workspace-mcp`, já documentado na 5.7), **Notion** novo (`@notionhq/notion-mcp-server`, oficial, `npx`, token único, confirmado ativo — versão 2.5.1 há poucos dias da pesquisa), **GitHub via Docker** novo (ver decisão seguinte). Pesquisado e **descartado** por ora: **Slack** — o pacote npm oficial antigo (`@modelcontextprotocol/server-slack`) foi descontinuado, e o substituto oficial é um server hospedado remotamente pela própria Slack (anunciado fev/2026), que exigiria transporte HTTP/OAuth no client MCP (hoje só stdio) — registrado como P25 em `PENDING.md`, decisão explícita do usuário de aceitar essa lacuna por ora em vez de integrar uma alternativa comunitária não verificada a fundo |
| MCP client — GitHub via Docker (Sessão 35) | O pacote npm oficial (`@modelcontextprotocol/server-github`) foi **arquivado** (mesmo destino do Google Drive, Sessão 26) — o substituto oficial ativo, `github/github-mcp-server`, roda via **Docker** (`docker run -i --rm -e GITHUB_PERSONAL_ACCESS_TOKEN ghcr.io/github/github-mcp-server`), não `npx`. Introduzir Docker como dependência de runtime quebra o princípio já estabelecido de evitar um segundo runtime opcional além do Node.js (P17) | **Aceito, opt-in via preset** ✓ — decisão explícita do usuário (pergunta feita: só Notion agora vs Notion + GitHub via Docker). Sem código novo: o preset gera um `McpServerConfig{command:"docker", args:["run","-i","--rm","-e","GITHUB_PERSONAL_ACCESS_TOKEN","ghcr.io/github/github-mcp-server"], env:{"GITHUB_PERSONAL_ACCESS_TOKEN":""}}` — o mecanismo genérico `[[mcp_servers]]` já suporta qualquer `command`, Docker incluso, sem mudança nenhuma em `warden-bootstrap`/`McpToolProvider`. `docker -e VAR` (sem `=valor`) repassa a env var do processo `docker` (que é quem `Command::envs` de fato seta) pro container — mesmo mecanismo de env já usado por todo `mcp_servers`, nada especial. Quem não tiver Docker instalado simplesmente não liga essa integração — nenhum outro preset depende dele |
| Warden como server MCP (P11b, Sessão 35) | Não iniciado até então. Transporte: stdio (processo local, lançado sob demanda por outro app, como todo client MCP já configura hoje) vs HTTP (listener persistente, exigiria expor uma porta) | **Novo binário `crates/warden-mcp-server`, stdio** ✓ — decisão do agente, consistente com "servidor é opcional" (ver mapa de dependência de servidor mais acima neste arquivo): um processo lançado sob demanda pelo client MCP que conecta (mesmo `command`/`args` que qualquer entrada de `mcpServers` de terceiro, ex. Claude Desktop) não é a topologia servidor↔cliente da Fase 9, é só mais um processo local. `Orchestrator` ganhou um getter novo, `tools() -> &[Arc<dyn Tool>]` (mesmo espírito do `vault()` já existente) — o binário chama `bootstrap()` normalmente (constrói o mesmo conjunto de tools que qualquer canal teria: vault, shell se habilitado, MCP servers configurados) e re-expõe esse conjunto via `ServerHandler` do próprio `rmcp` (mesma API server-side já provada em `warden-core/tests/mcp_stdio.rs`, agora usada em produção pela primeira vez — `rmcp` ganhou as features `server`+`transport-io` como dependência real, não só de teste). É um bridge puro: `list_tools` traduz `ToolSpec` pro formato MCP, `call_tool` despacha pro `Tool::call` já existente e serializa o resultado de volta como texto — nenhuma capacidade nova, só uma nova porta de entrada pras que já existem. Verificado de ponta a ponta com um client MCP real (`McpToolProvider`, o lado client já existente) conectando no binário real via subprocesso, listando as tools e fazendo um round-trip real de `write_file`/`read_file` |
| Canal WhatsApp (Fase 3) — protocolo IPC | Forma exata das mensagens JSON entre sidecar e core | Uma linha JSON por evento/comando. Sidecar → Rust (stdout): `{"type":"connected"}`, `{"type":"disconnected","loggedOut":bool}`, `{"type":"message","chatId":...,"senderName":...,"text":...\|null}` (`text: null` = mensagem sem corpo de texto legível, imagem/áudio/documento/etc. — é o que aciona a degradação graciosa da etapa 3.7). Rust → sidecar (stdin): `{"type":"send","chatId":...,"text":...}`. **QR code não entra no protocolo** — vira um arquivo PNG, caminho logado no **stderr** do sidecar (canal separado do stdin/stdout usado pro IPC), Rust só herda esse stderr (`Stdio::inherit()`). Ver decisão específica de renderização do QR abaixo |
| Canal WhatsApp (Fase 3) — código JS no repo | Onde/como versionar o script Node que fala com o Baileys — primeira vez que o projeto **escreve e versiona** código JS (tudo antes era `npx` contra pacotes de terceiros: Tavily, filesystem, Google Workspace) | `sidecar/whatsapp/` na raiz do repo (irmão de `desktop/`, outro subtree não-Rust com `package.json` próprio) — `package.json` + `index.mjs` puro (ESM, sem TypeScript/build step, já que é só cola fina em cima do Baileys). `baileys@^7.0.0-rc14` — a linha estável `6.7.24` (`dist-tags.legacy` no npm) era a intenção original, mas seu `libsignal` é resolvido via `git+https://...` em vez do registry npm, e o ambiente de verificação bloqueia fetch de dependências git; `7.0.0-rc14` resolve `libsignal` como dependência normal do registry e instalou/conectou de verdade. Trade-off aceito conscientemente: é uma pre-release pré-1.0, não a tag `latest`-estável — revisitar se uma `7.x` de verdade sair ou se `6.x` passar a instalar no ambiente do usuário. **Setup manual, sem automatizar**: `npm install` dentro de `sidecar/whatsapp/` antes do primeiro uso — mesmo espírito de não auto-instalar dependências de runtime já usado pro Node/npx em geral (P17). Verificado de ponta a ponta contra os servidores reais do WhatsApp (não mockado, e depois confirmado de novo pelo usuário na própria máquina): `warden-whatsapp` bootstrapa, spawna o sidecar de verdade, o sidecar conecta e gera um QR de pareamento real |
| Canal WhatsApp (Fase 3) — renderização do QR | `qrcode-terminal` (ASCII no terminal) vs `qrcode` (arquivo PNG) | **PNG** ✓ — decisão do agente, revisada depois de feedback real do usuário: a primeira versão usava `qrcode-terminal` com o truque de meio-bloco Unicode pra "comprimir" o QR verticalmente, assumindo uma proporção de fonte de terminal específica (~2:1 altura:largura); quando essa suposição não bate o QR sai visivelmente esticado — uma câmera genérica tolera a distorção, o scanner do próprio WhatsApp não. Trocado por `qrcode` (`QRCode.toFile`), que gera um PNG de verdade (512×512, pixels quadrados, sem depender de fonte nenhuma) salvo em `<authDir>/qr.png`, com o caminho logado no stderr do sidecar. Verificado de ponta a ponta: PNG válido gerado (`file` confirma 512×512 RGBA), stdout continua limpo (0 bytes) |
| Canal WhatsApp (Fase 3) — tratamento de mídia (etapa 3.7) | Degradação graciosa vs suporte multimodal de verdade | **Só degradação graciosa** ✓ — decisão explícita do usuário. Suporte de verdade (o modelo "entender" imagem/áudio) exigiria mudar `ModelProvider`/`Message` (`warden-core`) pra multimodal — mudança de model-layer que toca os dois providers (OpenAI, Gemini), não uma mudança de canal. Registrada como pendência nova (P21 em `PENDING.md`) pra uma sessão à parte |
| Canal WhatsApp (Fase 3) — trait `Channel` (fecha P20) | Com dois canais reais agora (Telegram HTTP long-polling, WhatsApp sidecar Node+IPC), P20 perguntava se valia revisitar a trait `Channel` adiada na Fase 2 | **Continua sem trait** — o que de fato é compartilhável entre os dois já está extraído em `handle_turn` (`warden-bootstrap`); os loops de recebimento em si (`TelegramApi::get_updates` via poll HTTP com offset vs `WhatsAppSidecar::recv_event` via stream de eventos sobre stdio, `&self` vs `&mut self`) continuam genuinamente diferentes o bastante pra uma trait `Channel` não reduzir duplicação real — só forçaria os dois loops numa assinatura comum sem corpo compartilhado. P20 fechada com essa conclusão; reabrir se um terceiro canal mostrar um padrão diferente |
| UX do `warden-cli` (P8) — nível de ambição | Cores+markdown+histórico vs + streaming de resposta vs TUI completo (`ratatui`) | **Cores+markdown+histórico** ✓ — decisão explícita do usuário, entre 3 níveis apresentados. Sem streaming (exigiria mudar `ModelProvider`/`Response` em `warden-core`, código que Telegram/WhatsApp também usam) e sem tela alternativa tipo `ratatui` (caixa de input fixa, painel próprio) — a tela continua rolando normalmente como hoje, só com output mais rico |
| UX do `warden-cli` — libs escolhidas | `termimad` (renderiza o markdown da resposta), `rustyline` (edição de linha com histórico persistido em disco), `indicatif` (spinner "Thinking..." enquanto espera o modelo) — todas greenfield, nenhuma lib de terminal existia no projeto antes disso. Cor do texto de uso de tokens: intenção original era reusar `crossterm` como dependência transitiva do `termimad`, mas checagem do `Cargo.toml` real do `termimad@0.35` mostrou que ele não depende mais de `crossterm` diretamente (usa `crokey`/`coolor`) — trocado por **`owo-colors`**, uma dependência dedicada e mínima só pra isso, em vez de depender de algo transitivo e frágil | — |
| UX do `warden-cli` — detecção de TTY | Os 4 testes de processo existentes (`tests/cli.rs`) rodam com stdin/stdout pipados (`Stdio::piped()`), não um terminal de verdade — `rustyline` em modo raw tipicamente não funciona direito nesse cenário | **`std::io::IsTerminal`** (na std desde Rust 1.70, sem dependência nova) ✓ — quando stdin não é um TTY de verdade, cai no loop simples de sempre (`run_plain`, inalterado), preservando os 4 testes existentes sem precisar tocar neles e mantendo o `warden` scriptável/pipável. Verificado de ponta a ponta com um TTY de verdade via `script` (aloca um pseudo-terminal): confirmado que o caminho rico é o que de fato roda (prompt, spinner animando, histórico), e que `exit` encerra limpo |
| MCP client — transporte HTTP (fecha P25, Sessão 36) | A decisão original da 5.2 (linha acima) adiava HTTP "pra quando surgir um caso real" — surgiu na Sessão 35 com o server hospedado oficial do Slack, remoto-only. `rmcp` v3 expõe isso via feature `transport-streamable-http-client-reqwest` | **`McpToolProvider::connect_http(name, url, headers)`** novo em `warden-core/src/tool/mcp.rs`, ao lado do `connect_stdio` já existente — mesma interface `ToolProvider`/`Tool` depois de conectado, o resto do orchestrator não sabe qual transporte um provider usa. Implementado com `StreamableHttpClientTransport::from_config(...)` (não `with_client` com nosso próprio `reqwest::Client`) — ver decisão de dependência abaixo pra por quê |
| MCP client — transporte HTTP, dependência `reqwest`/TLS (Sessão 36) | `rmcp` v3.1's `transport-streamable-http-client-reqwest` depende da sua **própria** cópia de `reqwest` — mas na versão `0.13`, uma major diferente do `reqwest 0.12` que o resto do Warden já usa (`OpenAiProvider`/`AnthropicProvider`), então as duas coexistem no grafo de dependências como crates distintas, sem unificação de features possível. Além disso, o `reqwest 0.13` não expõe mais um backend rustls+`ring` puro-Rust como feature própria — sua única feature `rustls` liga `aws-lc-rs` (biblioteca C compilada via `cmake`/`cc`), diferente do `reqwest 0.12` (que ainda usa `ring`, sem toolchain C nenhum) | **Aceito** ✓ — decisão do agente (baixo risco, mesmo espírito do trade-off do `npx`/Node.js em P17): reimplementar o transporte HTTP do zero sobre o `reqwest 0.12` do próprio Warden (a trait `StreamableHttpClient` do `rmcp` permite um client customizado, ver doc do `mcp.rs`) evitaria o `cmake`/`aws-lc-rs`, mas é retrabalho substancial (SSE parsing, gerência de sessão) só pra não ter uma segunda versão do `reqwest` no grafo — não vale o custo agora. Efeito prático: builds (inclusive cross-compilação futura pra Fase 7/mobile) agora precisam de um compilador C + `cmake` disponíveis, não só Rust puro. Revisitar se isso virar um problema real de build em CI/cross-compile |
| MCP client — HTTP, formato de config (fecha P25, Sessão 36) | `McpServerConfig` (TOML `[[mcp_servers]]`) precisa suportar as duas transportes sem quebrar `config.toml` já escritos (que não têm nenhum campo `transport`) | **Enum `#[serde(untagged)]`** ✓ — `McpServerConfig::Stdio{name,command,args,env}` (a mesma shape de sempre — nenhuma tag nova) vs `McpServerConfig::Http{name,url,headers}`, discriminados puramente pela presença de `command` vs `url` (não por uma tag `transport` explícita). Motivo: um `config.toml` da 5.2 pra cá continua parseando sem tocar em nada, e é exatamente o mesmo campo (`url`) que a UI do desktop usa pra decidir qual metade do card mostrar (ver decisão de UI abaixo). Confirmado com `toml`/`serde_json` (round-trip testado nos dois formatos, `warden-bootstrap/src/lib.rs` `parses_http_mcp_server_from_toml`/`save_config_round_trips_through_load_config`) |
| MCP client — HTTP, modelo de autenticação (Sessão 36) | Servers HTTP reais tipicamente exigem auth — cliente OAuth completo (dynamic client registration, browser consent, refresh token) vs headers estáticos configurados pelo usuário | **Headers estáticos** ✓ (`headers: Vec<(String,String)>` em `connect_http`, tipicamente `{"Authorization":"Bearer <token>"}`) — decisão do agente: um cliente OAuth de verdade é uma peça grande (fluxo de browser, storage de refresh token, renovação) fora do escopo desta sessão. Cobre bem qualquer server que aceite um token de longa duração colado à mão (é assim que a maioria dos servers MCP HTTP hoje se autentica além do OAuth). **Não cobre o Slack**: pesquisado nesta sessão, o endpoint oficial (`https://mcp.slack.com/mcp`, confirmado via busca) exige o fluxo OAuth completo — colar um token estático nesse endpoint não necessariamente funciona, a menos que o usuário já tenha um token minerado por outro client OAuth-capable. O preset "Slack" na UI (ver decisão de UI abaixo) é oferecido mesmo assim, com aviso explícito nesse sentido — P25 fica **parcialmente** resolvida: o transporte em si funciona (testado de ponta a ponta contra um server HTTP real, ver `crates/warden-core/tests/mcp_http.rs`), mas "conectar no Slack com um clique" continua bloqueado até (e se) um client OAuth for implementado |
| MCP client — HTTP, UI do desktop (Sessão 36) | `McpServerCard` (Settings) só sabia renderizar comando/args/env | **Select "Transport" no topo do card** (`stdio` vs `http`) que troca a forma do objeto (`{command,args,env}` ↔ `{url,headers}`) — mesmo padrão já usado pelo tipo de provider (`ProviderCard`). Env vars e headers compartilham o mesmo componente de lista chave/valor (`KeyValueListField`, extraído nesta sessão) já que são a mesma forma (`Record<string,string>`, linhas dinâmicas, valor mascarado). Novo preset "Slack (hosted — needs a bearer token)" pré-preenche a URL confirmada e um placeholder de header `Authorization: Bearer `, com o aviso de OAuth (decisão acima) no comentário do preset |
| MCP client — OAuth pro transporte HTTP (fecha P26, Sessão 37) | A Sessão 36 deixou o transporte HTTP só com headers estáticos, sem cobrir servers que exigem OAuth de verdade (Slack incluso). Implementar o fluxo (discovery RFC 9728/8414, Dynamic Client Registration, PKCE, browser consent, refresh) do zero seria uma peça grande — mas `rmcp` v3.1.2+ (a versão travada era 3.1.0) já embute um client OAuth completo atrás da feature `auth`, incluindo um adapter `AuthClient<C>` que pluga direto no transporte streamable-HTTP e renova token sozinho em qualquer 401 | **Usar o client OAuth do próprio `rmcp`** ✓ — `cargo update -p rmcp` (3.1.0 → 3.1.4, sem tocar `Cargo.toml`, que já pedia `"3"`), feature `auth` adicionada. Novo módulo `crates/warden-core/src/tool/mcp_oauth.rs`: `connect_http_oauth` (caminho headless, todo `bootstrap()`) e `authorize_interactively` (caminho interativo, botão "Connect" do desktop) são só glue sobre `AuthorizationManager`/`OAuthState`/`AuthClient` do `rmcp` — nenhuma mecânica OAuth reimplementada na mão. `McpToolProvider` ganhou um construtor `pub(crate) fn from_session` pra esse módulo poder produzir o mesmo tipo que `connect_stdio`/`connect_http` já produzem, sem duplicar `tools()`/`call()` |
| MCP client — OAuth, segunda dependência `reqwest` (Sessão 37) | `AuthClient::new(http_client, manager)` (a peça do `rmcp` que precisa ser plugada no transporte) exige um valor de verdade do tipo `reqwest::Client` — mas `rmcp` não reexporta esse tipo em nenhum caminho público, então só dá pra nomeá-lo dependendo da mesma versão exata (`0.13.2`) que o `rmcp` já usa internamente | **Aceito, mesma linha do trade-off da Sessão 36** ✓ — `reqwest-oauth = { package = "reqwest", version = "0.13.2" }` novo em `warden-core/Cargo.toml` (só usado dentro de `mcp_oauth.rs`), Cargo unifica automaticamente com a cópia que o `rmcp` já traz — não é uma terceira versão no grafo, só um segundo nome apontando pra a mesma que já existia desde a 36. Confirmado que compila e os testes passam com essa dependência extra |
| MCP client — OAuth, onde guardar o token (Sessão 37) | Um `CredentialStore` (trait do `rmcp`) precisa persistir o token entre reinícios do app — em memória (`InMemoryCredentialStore`, o default do `rmcp`) não serve, perde tudo a cada restart | **Um arquivo JSON por server**, `~/.config/warden/mcp_oauth/<nome-sanitizado>.json` — `FileCredentialStore` novo (`mcp_oauth.rs`) implementa `CredentialStore::load/save/clear` sobre `tokio::fs` + `serde_json` em cima de `StoredCredentials` (já `Serialize`/`Deserialize` no `rmcp`). Texto puro, mesma postura de segurança que toda outra credencial do Warden hoje (API keys em `config.toml`) — não é uma categoria nova de risco, só o mesmo padrão aplicado a um tipo novo de segredo. Nome sanitizado (fora de `[a-zA-Z0-9_-]` vira `_`) — colisão entre nomes só diferentes por pontuação é "problema do usuário resolver", mesma postura já tomada pra `ProviderConfig.id` |
| MCP client — OAuth, captura do redirect do browser (Sessão 37) | O passo interativo precisa capturar o `code`/`state` que o browser recebe de volta do authorization server — como fazer isso sem um servidor web de verdade rodando o tempo todo | **Listener TCP local, uma única request** — `authorize_interactively` faz `bind("127.0.0.1:0")` (porta efêmera do SO) antes de registrar o client (pro `redirect_uri` já sair certo na Dynamic Client Registration), aceita exatamente uma conexão, lê só a request line pra extrair a query string, responde uma página HTML estática de "pode fechar essa aba", encerra. Implementado à mão sobre `tokio::net::TcpListener`/`AsyncBufReadExt` (não `axum`) — não vale promover `axum` (hoje só dev-dependency, usado nos testes) a dependência de produção só pra servir uma única request fire-and-forget |
| MCP client — OAuth, escopo desta sessão (Sessão 37) | Dynamic Client Registration cobre o caso comum, mas nem todo authorization server suporta (alguns só aceitam um `client_id` pré-cadastrado fora de banda) | **Só DCR nesta passada** ✓ — decisão do agente, escopo mínimo: `AuthorizationRequest::with_preregistered_client` já existe no `rmcp` se isso for necessário depois (ex. se o Slack real não suportar DCR na prática — só descobrível testando contra o server de verdade, o que não foi feito automaticamente nesta sessão, ver `PENDING.md`). Sem UI de client_id/secret manual por enquanto |
| Desktop — reformulação visual do chat (Sessão 39) | Usuário: UI "não tá com cara de IA, parece mais um chat basicão" — pediu puxar mais pro estilo ChatGPT | **Restyle completo do chat**, mantendo a identidade roxa já decidida (Sessão 2026-08-02) e o sistema de tokens `--color-*` light/dark já existente (não trocado por um novo design system): mensagens do assistente perderam a caixa/borda — texto solto na página com um avatar circular (a marca "escudo" do Warden, SVG novo em `components/Icons.tsx`), só a mensagem do usuário continua em bolha; coluna de conversa centralizada (`max-width: 46rem`) em vez de ocupar a largura toda; composer virou uma pílula flutuante com botão de enviar circular (ícone, não mais texto "Send"); indicador de "pensando" novo (3 pontos animados) enquanto `isSending` — antes não existia nenhum feedback visual de "carregando"; sidebar ganhou a marca (logo + "Warden") no topo e Settings movido pro rodapé (link, não mais um ícone solto no canto); empty-state virou um "hero" centralizado com logo + saudação, no lugar do texto plano de antes. Ícones são SVG à mão (`Icons.tsx`) em vez de uma lib de ícones nova — mesmo espírito de dependência mínima do resto do projeto | Sessão 39 (2026-08-31) |
| Desktop — verificação da reformulação visual (Sessão 39) | Não dá pra ver o rótulo `npm run tauri dev` nativo (Wayland/KDE, sem `xdotool`/`wtype` funcional pra simular ou capturar essa janela especificamente — limitação já registrada em sessões anteriores) | **Screenshot via Playwright headless contra o próprio dev server do Tauri** (`http://localhost:1420`, a mesma URL que a janela nativa carrega, já que o Tauri é só um WebKitGTK apontado pra ali) ✓ — cobre layout/CSS puro (sidebar, empty state, composer, temas claro/escuro); mensagens com conteúdo real não dá pra testar assim (`invoke()`/IPC do Tauri não existe fora do webview nativo), então usei um HTML estático à parte, reaproveitando o `App.css` de verdade, só pra validar a bolha do usuário/avatar do assistente/markdown/indicador de "pensando" juntos. A janela nativa que o usuário já tinha aberta atualiza sozinha via Vite HMR — ele confirma visualmente o resultado final ali, não precisei pedir pra reabrir nada |
| Anexo de imagem no chat (fecha metade de P28, Sessão 40) | `Message`/`ModelProvider` só carregavam `content: String` — nenhum dos três providers falava multimodal. Escopo: só imagem (png/jpeg/webp/gif) vs também documentos genéricos (PDF/etc), decisão explícita do usuário — provedores tratam PDF/doc de forma desigual demais (OpenAI exige upload via Files API separada; Anthropic/Gemini aceitam inline parecido com imagem, mas não é o mesmo mecanismo que imagem) | **Só imagem, base64 inline, mesmo mecanismo nos 3 provedores** ✓ — `warden_core::model::Attachment{ mime_type, data }` novo, `Message` ganha `attachments: Vec<Attachment>` (só relevante em `Role::User`) e o construtor `user_with_attachments`. Cada provider serializa isso no formato próprio: OpenAI vira um array de partes `[{type:"text"},{type:"image_url", image_url:{url:"data:<mime>;base64,<data>"}}]` no lugar da `content: String` de sempre (só quando há anexo — mensagem sem anexo continua serializando como string simples, campo `Content` ganhou `#[serde(untagged)]` pra escolher a forma certa); Anthropic ganha um `ContentBlock::Image{source:{type:"base64", media_type, data}}` antes do bloco de texto; Gemini ganha um `Part.inlineData{mimeType, data}` antes da part de texto. Nenhum dos três providers tinha testes HTTP-mockados até agora (nenhuma infra de mock nesse nível no projeto) — verificação ficou em testes unitários puros de serialização (`serde_json::to_value` + assert nos campos-chave), não em uma chamada de API de verdade |
| Anexo de imagem — raio de mudança no `Orchestrator` (Sessão 40) | `handle_message` é usado por CLI/Telegram/WhatsApp/`DelegateTool`/testes — nenhum desses ganha UI de anexo nesta rodada, só o desktop | **Método novo em vez de mudar a assinatura existente** ✓ — `handle_message_with_attachments(history, user_input, attachments)` novo; `handle_message` vira um wrapper fino que chama o novo com `Vec::new()`. Só o comando `send_message` do desktop (`desktop/src-tauri/src/lib.rs`) usa o novo método diretamente — zero mudança de comportamento pros outros canais |
| Anexo de imagem — persistência (Sessão 40) | Conversas do desktop são um JSON por conversa (`ConversationMessage`, espelhado 1:1 com o `ChatMessage` do frontend) — se a imagem não for persistida, ela desaparece do histórico assim que a conversa recarrega, e o modelo perde o contexto visual em qualquer volta seguinte | **Base64 completo dentro do JSON da conversa** ✓ — `ConversationMessage.attachments: Vec<Attachment>` (`#[serde(default)]`, mesma retrocompatibilidade do `usage` opcional já existente), `ChatMessage.attachments?` no frontend. Trade-off aceito conscientemente: cada imagem anexada infla o arquivo da conversa em bytes proporcionais ao tamanho da imagem (base64 é ~33% maior que o binário) — sem limite de tamanho de arquivo/compressão nesta rodada; se isso incomodar no uso real, é candidato a um limite de tamanho no `read_attachment` do lado Rust |
| Anexo de imagem — leitura do arquivo (Sessão 40) | O composer precisa ler bytes de um arquivo local escolhido no diálogo nativo (`@tauri-apps/plugin-dialog`, já usado pro vault path) e mandar pro backend | **Novo comando Tauri `read_attachment(path)`**, não o plugin `fs` do frontend — lê via `std::fs::read` no lado Rust, detecta mime pela extensão (rejeita qualquer coisa fora de png/jpg/jpeg/webp/gif com "Unsupported file type" em vez de tentar mandar pro provider) e devolve já em base64 (`base64.workspace = true`, novo em `Cargo.toml` raiz — a versão 0.22 já era dependência transitiva via `reqwest`, só faltava declarada direto). Evita adicionar `@tauri-apps/plugin-fs` como dependência nova só pra isso |
| Input de voz — transcrição universal vs áudio nativo por provider (metade "input" de P28, Sessão 41) | Gemini e a API mais nova da OpenAI aceitam áudio nativo (bytes direto pro modelo, sem transcrição prévia); Anthropic não tem essa capacidade | **Sempre transcreve antes, nunca áudio nativo** ✓ — decisão explícita do usuário: funciona igual não importa qual dos 3 provedores de chat está ativo. Zero mudança em `Message`/`ModelProvider`/nos três providers desta vez (ao contrário do anexo de imagem, Sessão 40) — o áudio nunca chega na abstração multimodal de chat, é resolvido inteiramente antes, virando texto puro |
| Input de voz — serviço de transcrição (Sessão 41) | Precisa de um serviço de STT de verdade — não existe embutido em nenhum SO/browser numa qualidade aceitável | **Whisper da OpenAI** (`POST https://api.openai.com/v1/audio/transcriptions`, `model: "whisper-1"`) ✓ — decisão do agente, mesma lógica do `MAX_TOKENS` fixo do `AnthropicProvider`: endpoint STT mais estabelecido do mercado, escopo mínimo (sem config de modelo ainda), revisável depois se `gpt-4o-transcribe` compensar trocar. `reqwest` ganhou a feature `multipart` (só tinha `json`+`rustls-tls`) |
| Input de voz — chave dedicada vs reaproveitar provider OpenAI (Sessão 41) | O usuário pode ter Gemini/Anthropic como provider de chat ativo e mesmo assim querer voz — Whisper é sempre OpenAI, não importa o provider de chat escolhido | **Chave própria, mesmo padrão do Tavily** ✓ — `ApiKeys.whisper: Option<String>` novo (`crates/warden-bootstrap`), independente do registry `ProviderConfig` (Sessão 35). Settings ganhou "Whisper API key (voice input)" como um segundo `ApiKeyField`, sem reaproveitar a key de um provider OpenAI já cadastrado (evita acoplar "quero voz" a "preciso ter OpenAI configurado pro chat") |
| Input de voz — onde vive o código de transcrição (Sessão 41) | Não é um `ModelProvider` (não conversa) nem um `Tool` do orchestrator (o modelo não decide invocar isso — é pré-processamento antes de `handle_message` sequer rodar) | **Módulo solto `crates/warden-core/src/transcribe.rs`**, não dentro de `model/` nem `tool/` ✓ — `transcribe_audio(api_key, bytes, filename)` puro, chamado direto pelo comando IPC `transcribe_audio` do desktop, sem passar pelo `Orchestrator` |
| Input de voz — gravação no composer (Sessão 41) | Precisa capturar áudio do microfone dentro do webview Tauri (WebKitGTK no Linux) | **Tentativa 1: `MediaRecorder`/`getUserMedia` do browser** — Web API padrão, sem plugin/permissão Tauri nova. **Revertido na mesma sessão**: confirmado contra a janela real que o WebKitGTK nega toda chamada de `getUserMedia` por padrão (`NotAllowedError`) — Tauri/wry nunca conecta o sinal `permission-request` que o WebKit exige pra sequer perguntar, uma limitação conhecida e sem solução oficial do próprio time do Tauri (issues [#12547](https://github.com/tauri-apps/tauri/issues/12547), [#8346](https://github.com/tauri-apps/tauri/issues/8346)). **Solução final: captura nativa via crate `cpal`** ✓ — usuário escolheu isso em vez do hack GTK-específico de conectar o sinal na mão (que o próprio Tauri classifica como abaixo do padrão de segurança deles, e só resolveria no Linux). Contorna o webview por completo; funciona igual em Linux/Windows/macOS. Ver decisão seguinte pro desenho |
| Input de voz — arquitetura da captura nativa (Sessão 41) | `cpal::Stream` não é `Send` de forma confiável em todos os backends de plataforma — não dá pra guardar direto no `AppState` (`Mutex` compartilhado, acessado de qualquer thread do runtime async do Tauri) | **Thread dedicada dona do stream** ✓ — novo módulo `desktop/src-tauri/src/recording.rs`: `start()` abre o dispositivo default (`cpal::default_host().default_input_device()`), sobe uma thread OS própria que constrói o `Stream`, chama `.play()`, e bloqueia num `mpsc::Receiver` até `stop()` sinalizar — o `Stream` nunca sai dessa thread. `start()` só retorna depois que a thread confirma que o device abriu de verdade (canal de "pronto" separado), então erro de "sem microfone" aparece na hora do clique, não só no stop. Callback de captura faz downmix de qualquer formato que o cpal devolva (F32/I16/U16, qualquer contagem de canais) pra mono `i16`, direto no buffer compartilhado (`Arc<Mutex<Vec<i16>>>`) — único jeito de extrair os samples de volta é a thread devolver o `Vec` como retorno do `JoinHandle` quando `stop()` faz `.join()`. Codificado como WAV (`hound`, mono 16-bit) e devolvido como o mesmo `AttachmentPayload` que a imagem já usa — `transcribe_audio` (Sessão 41, parte 1) não mudou nada, só passou a receber `audio/wav` em vez de `audio/webm` |
| Input de voz — texto cai revisável, não manda sozinho (Sessão 41) | Uma transcrição pode sair errada (ruído, sotaque, sigla mal entendida) | **Preenche o campo de mensagem, usuário confirma o envio** ✓ — decisão do agente, mesma cautela que já rege o resto do composer (anexo também exige clique explícito em "Send"). Rejeitado: auto-enviar direto a transcrição sem revisão |
| TTS na resposta — serviço e chave (fecha a última metade de P28, Sessão 42) | Simetria com o input de voz (Sessão 41): precisa de um serviço de TTS de verdade, e o usuário já tem uma chave OpenAI dedicada (`ApiKeys.whisper`) cadastrada só pra voz | **Mesma conta OpenAI, chave reaproveitada** ✓ — decisão explícita do usuário: em vez de um segundo campo `tts` em `ApiKeys`, `POST /v1/audio/speech` (`model: "tts-1"`, `voice: "alloy"`, fixos por enquanto, mesmo espírito do `whisper-1` fixo) usa a mesma `api_keys.whisper` já existente — ambos endpoints de áudio da mesma conta OpenAI. Novo módulo `crates/warden-core/src/speech.rs`, espelhando `transcribe.rs`, mas devolvendo os bytes crus do mp3 direto (`response.bytes()`) em vez de desserializar um envelope JSON — o endpoint de fala não tem um |
| TTS na resposta — acionamento manual, não autoplay (Sessão 42) | Toda resposta nova podia ser lida automaticamente (mais "assistente de voz de verdade") ou só sob demanda | **Botão por mensagem, manual** ✓ — decisão explícita do usuário, mesmo espírito do resto do composer (nada acontece sozinho: anexo exige clique em enviar, transcrição de voz cai no campo pra revisão, não manda sozinha). `MessageBubble.tsx` ganhou um botão (`SpeakerIcon`/`StopIcon`) junto da contagem de tokens, com três estados (idle/loading/playing); clicar durante a reprodução para e reseta pro início, não pausa/retoma |
| TTS na resposta — sanitização de markdown antes de sintetizar (Sessão 42) | O texto da resposta do assistente é markdown renderizado (`ReactMarkdown`) — mandar isso cru pro TTS faz o `tts-1` "ler" asteriscos, colchetes de link e crases em voz alta | **Util novo `desktop/src/lib/stripMarkdown.ts`, baseado em regex** ✓ — decisão do agente: cobertura mínima suficiente pro que uma resposta de modelo realmente produz (cabeçalhos, ênfase, code fences/inline code, links, listas), sem promover `remark`/`strip-markdown` a dependência nova só pra isso — mesmo espírito de dependência mínima do resto do projeto (`Icons.tsx` à mão em vez de lib de ícones, Sessão 39) |
| Agentes nomeados — onde a persona vive (fecha P3, Sessão 43) | Vault (arquivo markdown por agente, no espírito da "memória") vs `config.toml` (registry, no espírito dos providers) | **`config.toml`, registry** ✓ — decisão explícita do usuário: mesmo padrão já usado pros providers de modelo (Sessão 35) — `AgentConfig { id, persona, provider_id }` novo em `warden-bootstrap`, `id` dobra como nome de exibição (mesma convenção de `ProviderConfig`), `persona` é texto livre sem nenhuma estrutura imposta (o pedido explícito do usuário foi só "um campo de texto"). Ao contrário dos providers, não existe um "agente ativo" global — uma conversa sem `agent_id` simplesmente roda sem persona, comportamento idêntico a antes dessa feature existir |
| Agentes nomeados — como a persona entra no prompt (fecha P3, Sessão 43) | `Orchestrator::handle_message`/`handle_message_with_attachments` não tinham nenhum conceito de system prompt | **`Orchestrator::handle_turn` novo, mais geral** ✓ — mesmo padrão de "método novo, os existentes viram wrapper fino" já usado quando `handle_message_with_attachments` apareceu (Sessão 40): `handle_turn(history, user_input, attachments, system_prompt: Option<&str>)` dá `push` da persona como a **primeira** mensagem (antes do bloco de contexto do vault) quando não vazia/só espaço — ignorada em branco pra um agente com persona vazia se comportar como "sem agente". Zero mudança pros 3 canais existentes (CLI/Telegram/WhatsApp) e pro `DelegateTool`, que continuam chamando as duas variantes de sempre (`None`) |
| Agentes/modelo por conversa — trocar sem re-rodar `bootstrap()` (Sessão 43) | O desktop guarda **um** `Orchestrator` no `AppState`, construído uma vez (ou recriado inteiro no save de Settings) — `bootstrap()` reconecta MCP servers, refaz OAuth, etc., caro demais pra rodar a cada mensagem só pra trocar de modelo | **`Orchestrator::with_model(model) -> Self` novo** ✓ — barato de propósito: `Orchestrator` já deriva `Clone`, `model`/`vault` são `Arc` e `tools` é `Vec<Arc<_>>`, então clonar é só bump de refcount. `send_message` (desktop) usa isso pra trocar só o modelo de uma chamada específica quando o `provider_id` escolhido difere do provider ativo — constrói o `ModelProvider` avulso via `build_model_provider` (antes `fn` privada em `warden-bootstrap`, agora `pub`), sem tocar em vault/tools/MCP |
| Agentes/modelo por conversa — escopo da troca (Sessão 43) | Trocar agente/modelo no meio de uma conversa podia reprocessar o histórico inteiro com a nova persona, ou só valer dali pra frente | **Só dali pra frente** ✓ — decisão explícita do usuário: o histórico já mostrado na tela não muda de dono. Naturalmente é o que já acontece: `history` é reconstruído do zero a cada `send_message` a partir do que está persistido, e a persona/modelo só afetam a chamada atual — nada precisou ser feito de propósito pra garantir isso, é uma consequência de como `handle_turn` já funciona (stateless entre chamadas) |
| Agentes/modelo por conversa — persistência da seleção (Sessão 43) | Reabrir uma conversa devia lembrar qual agente/modelo foi usado por último, ou sempre voltar pro default | **Gravado na própria conversa** ✓ — `Conversation` (`warden-bootstrap`) ganhou `agent_id`/`provider_id: Option<String>` (`#[serde(default)]`, mesma retrocompatibilidade de `usage`/`attachments`, Sessões 35/40), atualizado a cada `appendMessage` no frontend pra refletir a seleção corrente. Ao trocar de conversa, `App.tsx` reinicializa os seletores a partir desses campos — caindo pro default (sem agente / `activeProvider`) se o id salvo não bater com nada configurado agora (agente ou provider apagado depois) |
| Bug real — renomear um provider desincroniza `active_provider` (achado testando P32, Sessão 44) | Em Settings, o campo "Name" de um `ProviderCard` edita `provider.id` direto (`onChange={(e) => onChange({ ...provider, id: e.currentTarget.value })}`) — nada propagava esse rename pro `form.activeProvider` nem pro `agent.providerId` de nenhum agente que apontasse pra ele. O usuário bateu nisso ao vivo: criou um provider (id automático `provider-1`), renomeou pra `gemini`, `activeProvider` ficou órfão apontando pro nome antigo — `save_settings` aceitava sem reclamar (sem validação nenhuma de que `active_provider` bate com algum provider da lista) e só quebrava depois, na hora de mandar mensagem (`resolve_model_provider` em `warden-bootstrap`: `active_provider 'provider-1' not found among configured providers`) | **Dois fixes, um em cada ponta** ✓ — (1) `SettingsView.tsx`, `updateProvider`: quando o `id` de um provider muda, propaga o rename pro `activeProvider` (se apontava pro id antigo) e pro `providerId` de qualquer agente que referenciasse o id antigo, no mesmo `setForm`; (2) `desktop/src-tauri/src/lib.rs`, `save_settings`: validação nova, símile das já existentes de nome vazio/duplicado — se `active_provider` não é vazio, tem que bater com o `id` de algum provider da lista, senão erro claro na hora do Save (`active provider '...' is not one of the configured providers`) em vez de só na hora de mandar mensagem. O rename de um **agente** tem o mesmo padrão de bug em teoria (agent id editável do mesmo jeito), mas não foi tocado aqui — o efeito é mais brando (uma conversa com `agent_id` órfão só cai pro "sem agente" default, não quebra o envio, ver Sessão 43) e não foi o que o usuário bateu de verdade |
| Bug real — Gemini "thinking" rejeita `functionCall` sem `thoughtSignature` (achado testando P32, Sessão 44) | `gemini-3.5-flash` (o default atual, ver `default_model_for`) é um modelo "thinking" — a API anexa um `thoughtSignature` opaco à `Part` de um `functionCall` na resposta, e exige esse mesmo valor de volta na mesma posição quando aquele histórico de turno é reenviado; sem ele, `generateContent` responde 400 INVALID_ARGUMENT ("Function call is missing a thought_signature..."). O `ToolCall` compartilhado (`warden_core::model::ToolCall`, usado pelos 3 providers) não carregava esse dado — nenhuma chamada de tool sobrevivia a um segundo turno contra o Gemini. Bateu direto na primeira vez que o modelo chamou `delegate_task` (sub-agente) durante o teste de P32 | **`ToolCall` ganhou `thought_signature: Option<String>`** ✓ — sempre `None` pra OpenAI/Anthropic (não têm o conceito), populado só pelo `GeminiProvider`: `ResponsePart` (resposta recebida) ganhou o campo `thoughtSignature` (sibling de `functionCall`, não aninhado nele — confirmado lendo o formato real do erro/docs da API) e `chat()` propaga pro `ToolCall`; `to_content()` (montagem da próxima requisição) devolve esse valor no `Part` do `functionCall` reconstruído a partir do histórico. `skip_serializing_if` mantém o campo fora do payload quando `None` (nenhuma mudança de comportamento pra modelos não-thinking ou quando o campo nunca veio). 2 testes novos em `gemini.rs` cobrindo os dois casos (com e sem signature) |
| Streaming de verdade no `ModelProvider` (fecha parte de P8, Sessão 46) | Como adicionar streaming sem obrigar Telegram/WhatsApp/Desktop/`DelegateTool` a mudar — decisão de 2026-08-09 tinha rejeitado streaming justamente por isso | **`chat_stream` vira o único método obrigatório da trait; `chat()` vira um default que drena o stream** ✓ — `chat_stream(messages, tools) -> anyhow::Result<ChatStream>` (`ChatStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>`) é o que cada provider implementa de verdade; `chat()` (mesma assinatura de sempre) passou a ser um método **default** da trait que drena o stream inteiro num `Response` via `drain_chat_stream` (`model/mod.rs`, `pub(crate)`, compartilhado com o orchestrator). Resultado: `Orchestrator::handle_turn` (e por consequência Telegram/WhatsApp/Desktop/`DelegateTool`, que só chamam `handle_message`/`handle_turn`) **não mudaram nenhuma linha de código de produção** — só os 7 mocks de teste que implementavam `ModelProvider` direto precisaram trocar `chat` por `chat_stream` (mecânico, usando o novo `response_stream(Response) -> ChatStream` pra virar um stream de um item só). `StreamEvent` tem 3 variantes: `ContentDelta(String)`, `ToolCallDelta { index, id, name, arguments_delta, thought_signature }` (concatenado por índice e só parseado como JSON no fim, igual ao não-streaming de sempre) e `Usage(Usage)`. Gemini é um caso à parte: `FunctionCall.args` é um objeto JSON nativo no wire, nunca uma string — estruturalmente impossível chegar picotado, então o provider emite **um único** `ToolCallDelta` inteiro por function call (com `thought_signature` junto), sem exigir nenhum caso especial no acumulador. SSE via `eventsource-stream` (pequena, sem conflito de versão — só depende de `nom`) sobre `response.bytes_stream()` (`reqwest` ganhou a feature `"stream"`), construção de cada stream via `async_stream::try_stream!`. Gotcha real documentado: sem `stream_options.include_usage: true` no request da OpenAI, `usage` simplesmente some em modo streaming — corrigido explicitamente. Anthropic reporta `usage` partido em dois eventos (`message_start`/`message_delta`), combinados antes de emitir. Novo caminho de entrada pra quem quer consumir o streaming de verdade: `Orchestrator::handle_message_streaming`/`handle_turn_streaming(..., on_event)` — `handle_turn` normal é literalmente isso com um sink no-op, uma só implementação do loop de tool-calling em vez de duas que pudessem divergir. Testes novos: parsing de SSE por provider (função privada testável direto com strings fabricadas, incluindo o teste de regressão da suposição do Gemini), `ResponseAccumulator` isolado, e `handle_turn_streaming`/`handle_message` provando paridade de comportamento com antes de existir streaming |
| Reescrita do `warden-cli` interativo com `ratatui` (fecha parte de P8, Sessão 46) | Como dar a "cara de Claude Code" sem alt-screen (decisão de não usar `ratatui` fullscreen continua válida — perderia o scrollback nativo do terminal) | **`ratatui` em modo `Viewport::Inline`, não fullscreen** ✓ — só a caixa de input e a caixa de "pensando" usam `ratatui` (`Terminal::with_options(..., TerminalOptions { viewport: Viewport::Inline(3) })`), cada uma com seu próprio `Terminal` de vida curta (criado e descartado — com `.clear()` explícito antes de sair — a cada troca de fase), então o resto do terminal rola normal. `rustyline` saiu (não dava pra coexistir com um loop de redraw dono da região) — entra um `LineEditor` próprio (`Vec<char>` + cursor UTF-8-aware, histórico com draft ao navegar pra cima/baixo), testado isoladamente sem terminal nenhum (10 testes novos). A resposta do modelo, assim que o primeiro `ContentDelta` chega, é impressa como **texto puro** via `print!`+flush conforme streama (sem re-renderizar markdown incrementalmente — problema bem maior, fora de escopo, e alinhado com o pedido de "menos foco em código"); a ponte entre o callback síncrono `on_event` do orchestrator e o loop assíncrono de render é um `mpsc::unbounded_channel`. Ctrl+C durante "pensando" ou durante o streaming aborta a chamada (`JoinHandle::abort()`) sem gravar a resposta parcial no histórico da conversa. Histórico de digitação (não confundir com histórico de conversa, que nunca foi persistido aqui) passou a ser um arquivo texto simples (uma linha por entrada), formato novo — não tenta ser compatível com o arquivo antigo do `rustyline`. `cargo build/test/clippy --workspace` limpos; **verificação visual numa janela real ainda não feita** (mesma limitação de sempre pra testar terminal raw-mode a partir daqui) — registrada como P34 |
| `StorageProvider`/`DecentralizedVaultProvider` — semântica de `read`/`write`/`list`/`delete` pro provider descentralizado (P61, Sessão 58) — **removido na Sessão 105** (P61: a memória fica sempre local e só sincroniza) | `pin()` não tem leitura seletiva e cada publicação no Arweave exige uma aprovação física (QR) no celular — uma trait de CRUD por arquivo não consegue chamar isso de verdade sem travar esperando um celular que nunca vê o QR. Opções levadas ao usuário: delegar tudo pro `Vault` local (`read`/`write`/`list`/`delete` nunca tocam Arweave) vs. cada escrita disparar um push automático (exigiria repensar o fluxo QR manual do zero) | **Delega pro `Vault` local — os 6 métodos de `DecentralizedVaultProvider` são idênticos aos de `LocalFSProvider`** ✓ (confirmado com o usuário antes de codar). A interface `StorageProvider`/`AuthProvider` existe (piso pedido pela spec pro MVP — `crates/warden-core/src/storage/mod.rs`, `export_all`/`import_all` com default construído sobre os 4 primitivos, mesmo padrão de `ModelProvider::chat` sobre `chat_stream`; nomeação `snake_case`, não `exportAll`/`importAll` como no português da spec), mas o push/pull real pra Arweave continua exclusivamente por `SyncEngine::begin_push`/`finish_push`/`pull` (já wireados nos comandos `/sync`) — nada nesses fluxos mudou. `Vault` ganhou `delete()` novo (antes só existia via `std::fs::remove_file` direto em `warden-sync`'s `bundle::apply_bundle`, ajustado pra usar `vault.delete`). `warden-bootstrap`: `StorageProviderKind` (campo único + `build_storage_provider`, não um registry como `providers` — só existe um backend ativo por instalação), `resolve_storage_provider` erra explicitamente num valor de env não reconhecido (diferente de `resolve_flag`/`resolve_delegate_max_depth`, permissivos — aqui um typo mudaria onde a memória mora). `resolve_vault_path` extraído de `bootstrap()` e reaproveitado pelo CLI (removeu uma duplicação) e corrigiu um bug real no desktop (`SyncEngine` sempre usava `desktop_default_vault_path()` fixo, ignorando `config.vault_path`). **Atualizado 2026-09-10 (Sessão 59)**: UI de escolha implementada — `desktop`'s `SettingsView.tsx` ganhou uma seção "Storage" com 4 cards de rádio (`local`/`decentralized_vault` selecionáveis, `remote_node`/`managed_cloud` com badge "Coming soon" e `disabled`), cada um com a explicação didática pedida pela spec, incluindo o aviso de que `decentralized_vault` **não** liga o backup Arweave sozinho (isso é só via Sync). Backend (`save_settings`) reaproveita `resolve_storage_provider` pra parsear a string do form e passou a gravar o valor escolhido de verdade em vez de só carregar adiante o que já tava em disco — mas ainda **rejeita** `remote_node`/`managed_cloud` mesmo assim (defesa em profundidade, já que o frontend também os bloqueia). **Atualizado 2026-09-10 (Sessão 59, continuação)**: fluxo real de migração implementado — **decisão de escopo confirmada com o usuário antes de codar**: já que `LocalFSProvider`/`DecentralizedVaultProvider` são construídos a partir do mesmo `Arc<Vault>`, migrar entre os dois hoje é um self-copy no-op; construído mesmo assim como motor genérico (pronto pro dia que `RemoteNodeProvider`/`ManagedCloudProvider` existirem). `warden_core::storage::migrate(from, to)` faz `export_all`→`import_all` e **reconfirma re-exportando de `to` e comparando byte-a-byte** com o snapshot original — `import_all`'s `Ok(())` só prova que nenhum `write` retornou erro, não que o conteúdo chegou íntegro. `desktop`'s `save_settings` chama isso sempre que `storage_provider` muda de tipo e o anterior é implementado, só persistindo a config nova se a migração passar (uma falha deixa `config.toml` intocado, ainda no provider anterior que continua funcionando). **Atualizado 2026-09-10 (Sessão 59, continuação 2)**: `AuthProvider` real implementado — bloqueio real encontrado e confirmado com o usuário antes de codar: **não existe assinatura/billing em lugar nenhum do TruthID hoje**, então "checar uma assinatura de verdade" não tem o que checar. `TruthIdAuthProvider` (`warden-sync`) mapeia `is_subscription_active` pra um **proxy de pareamento** (tem `owner_address` no `SyncManifest`?), documentado explicitamente como não sendo uma checagem de assinatura real; `login`/`logout` erram de propósito, já que a trait não recebe parâmetro nenhum mas o pareamento de verdade é QR-mediado e assíncrono. `build_auth_provider` em `warden-bootstrap` despacha por `StorageProviderKind` (só `DecentralizedVault` usa `TruthIdAuthProvider`, resto usa `NoAuthProvider`), mesma forma de `build_storage_provider`. **Ainda em aberto (antes desta atualização)**: como o push/pull QR-interativo poderia um dia se encaixar numa trait genérica (se é que deve), `RemoteNodeProvider`/`ManagedCloudProvider` (v2/v3), e uma checagem de assinatura de verdade — bloqueada até existir alguma infra de billing real pro TruthID. **Atualizado 2026-09-11 (Sessão 60)**: UI de Settings pro `remote_node` implementada, fechando a lacuna de UI do v2 — `RemoteNodeForm` novo em `SettingsView.tsx` (mesmo padrão condicional do campo "Base URL" do `ProviderCard`), 5 campos (`server_url`/`device_id`/`device_name`/`auth_key`/`target_device_id`) aparecendo quando `remote_node` está selecionado, que deixou de ter o badge "Coming soon". `save_settings` valida os 5 campos como tudo-ou-nada (nenhum preenchido, ou todos) antes de persistir, e só rejeita `managed_cloud` agora (não mais `remote_node` também). Bug real corrigido no caminho: o passo de migração (Sessão 59) usava `existing.remote_node` (a config antiga) pra construir o `to_provider` mesmo quando o destino novo era `remote_node` — deveria usar a config recém-digitada nesta mesma chamada (`config.remote_node`), senão a primeira troca pra `remote_node` tentaria conectar com uma config vazia/errada. `storage_provider_kind_is_implemented` (desktop) passou a incluir `RemoteNode` no conjunto "tem `StorageProvider` de verdade", então migrar pra fora dele agora passa pelo `export_all`/`import_all`/reconferência de verdade em vez de ser pulado. **Não verificado com um `warden-server`+`warden-node` reais rodando** (só a suíte automatizada já existente cobre esse caminho) nem com Chrome/Playwright real (extensão não conectada neste ambiente) — só `cargo check`/`clippy`/`tsc`/`npm run build` e revisão manual do diff. **Atualizado 2026-09-15 (Sessão 68)**: a lacuna QR-interativa fechada — descoberta chave antes de codar: só o *push* pro Arweave exige aprovação por celular (`PendingPin`/TruthID); o *pull* é inteiramente não-interativo (só GraphQL + decrypt local, travado apenas em `owner_address` existir). `StorageProvider` ganhou dois métodos novos com default que delega pros planos (`export_all_interactive`/`import_all_interactive`, `on_qr: Option<&(dyn Fn(String) + Send + Sync)>` recebendo o JSON cru do payload do QR — nunca um SVG renderizado, já que `warden-core`/`warden-sync` não dependem do crate `qrcode`, que só existe em `desktop/src-tauri`) e uma função irmã `migrate_interactive` (a re-verificação final continua usando `export_all` plano, não o interativo — nesse ponto os arquivos já foram escritos localmente, um export local simples já prova integridade sem repetir um pull à toa). `DecentralizedVaultProvider` ganhou um campo `sync: SyncEngine` de verdade (assinatura de `new` mudou, só 2 call sites) — `export_all_interactive` chama `sync.pull()` real quando inicializado+pareado (erro de pull propaga como falha dura, não cai pro export local stale); `import_all_interactive` escreve local e, se inicializado, chama `sync.begin_push()`/invoca `on_qr` com o payload/`sync.finish_push()` de verdade (erro real também propaga). Sem inicialização/pareamento, ambos degradam pro comportamento de sempre (sem erro, sem rede) — selecionar `decentralized_vault` sem nunca visitar a tela Sync continua funcionando, só sem publicar nada, mesmo comportamento de antes desta sessão. `warden-bootstrap`'s `build_storage_provider` monta o `SyncEngine` (mesmos helpers de path que o desktop já usa — `warden-sync` não pode depender de volta de `warden-bootstrap`). `desktop`'s `save_settings` ganhou `app: AppHandle`, a chamada de migração trocou pra `migrate_interactive` com uma closure que renderiza o SVG (`crate::qr::render_qr_svg`, reaproveitado) e emite `app.emit("migration-qr", ...)`; `SettingsView.tsx` escuta esse evento durante o save e mostra um modal com o QR (reaproveita o markup/CSS de QR que `SyncView.tsx` já tinha, só com um backdrop novo). Testado com fake gateway real (prova que `export_all_interactive` pulha um bundle mais novo antes do export, e que um erro real de pull propaga) e fake phone real (`import_all_interactive_with_hosts`, novo método só-teste que sweepa hosts fixos em vez do LAN real — prova que `on_qr` recebe o payload certo, que o push completa de verdade contra o celular falso, e que o manifesto em disco reflete isso depois). Bug real pego pelos próprios testes no caminho: dois testes que construíam o `SyncEngine` manual apontavam o vault e os arquivos de sync (`sync_secrets.json`/`sync_manifest.json`) pro **mesmo diretório** — depois de `init_fresh`, esses JSONs viravam "conteúdo não rastreado do vault" aos olhos do `diff_vault`, fazendo `begin_push` achar que tinha mudança real e tentar um push de verdade sem celular nenhum configurado (timeout de ~180s); corrigido separando os diretórios (mesmo padrão sibling que produção sempre usou). **Não verificável neste ambiente, mesmo assim**: um celular TruthID real escaneando um QR real e uma transação Arweave real de ponta a ponta — mesma lacuna de sempre. `cargo test --workspace`/`clippy --workspace --all-targets` limpos, `npx tsc --noEmit`/`npm run build` limpos. Ver `PENDING.md` P61 |
| `warden-server` — roteamento de tool call pra um dispositivo específico (Fase 9.3/9.4, Sessão 59, continuação 4) | Escolhido atacar `RemoteNodeProvider` (v2 do P61) — descoberto que não dava pra construí-lo sem antes existir um jeito de "dispositivo A pede pro servidor rotear uma chamada pro dispositivo B", que a Fase 7.4 não fazia (só round-trip pro mesmo dispositivo que anunciou a tool). Bifurcação real de arquitetura: estender o `warden-server` (registro de dispositivos + roteamento) vs. um protocolo P2P dedicado (mesmo espírito do LAN sweep do `warden-sync`) vs. só a interface sem transporte nenhum | **Estender o `warden-server`, opção escolhida pelo usuário** ✓ — `Server` ganhou `devices: Arc<Mutex<HashMap<String, RemoteToolChannel>>>`, populado no `Hello` de qualquer dispositivo (advertindo tools ou não — ser um alvo de roteamento não depende de ter tools Fase 7.4) e removido no fim da conexão (limitação aceita, não tratada: uma reconexão rápida correndo com a limpeza da conexão antiga podendo remover o registro novo — sem cenário de reconexão de verdade ainda pra isso importar). Protocolo novo: `ClientMessage::CallDeviceTool { call_id, target_device_id, tool, arguments }` e `ServerMessage::DeviceToolResult`/`DeviceToolError`. Peça-chave pra evitar colisão de `call_id`: **`RemoteToolChannel::call` virou o único alocador de call-id/mapa de pendências da conexão**, extraído do corpo que antes só existia dentro de `RemoteTool::call` — agora tanto a Fase 7.4 (modelo pedindo pra própria conexão rodar uma tool que ela anunciou) quanto o roteamento cross-device novo (uma conexão *diferente* pedindo a mesma coisa) compartilham o mesmo contador/mapa por conexão, então nunca competem pelo mesmo id. O servidor não inspeciona `tool`/`arguments` — só o código do lado do dispositivo-alvo decide o que significam (que ainda não existe, ver abaixo). Testado com dois `ServerConnection` reais na mesma suíte de integração já existente (`tests/device_routing.rs`, 4 testes: roteamento com sucesso, erro do lado do alvo repassado com a mensagem real, alvo nunca conectado, alvo que mandou `Goodbye` e foi desregistrado) — `cargo test -p warden-server` (27 testes) e `clippy --all-targets` limpos. **Escopo desta rodada, decisão explícita do usuário**: só o lado servidor (protocolo + registro + roteamento) — o `RemoteNodeProvider` (`StorageProvider`) em si e o cliente WS persistente que ele precisaria pra falar com essa rota nova (conexão de vida longa, Hello, reconexão — peça grande por si só, nada parecido existe hoje: só o mobile Dart e o CLI/desktop usam `warden-server`, e só pro papel de `Chat`) ficam pra uma próxima rodada. **Atualizado 2026-09-10 (Sessão 59, continuação 5)**: o lado que chama, implementado — `RemoteNodeProvider`/`RemoteNodeClient` (`crates/warden-server/src/remote_node.rs`), plano escrito e aprovado antes de codar (a pedido do usuário). Uma única task de fundo por conexão (`tokio::select!` entre canal de saída e `conn.recv()`, mais simples que o split sink/stream do `server.rs` — aqui nada exige responder a uma mensagem iniciada pelo servidor enquanto uma chamada está em voo, já que o protocolo só tem `Ping` do cliente/`Pong` do servidor, nunca o contrário), mesmo alocador de `call_id`/mapa de pendências de `RemoteToolChannel::call` reaproveitado no formato (não no código — `RemoteNodeClient` é sua própria struct, já que aponta pra um dispositivo-alvo fixo em vez de responder localmente). Contrato de wire fixo pras 4 operações (`vault_read`/`write`/`list`/`delete`, conteúdo em base64, `list` devolve `{"paths": [...]}`) documentado no código — é o que um agente-de-nó real (lado alvo) precisaria implementar igual. Testado contra um alvo roteirizado (`ServerConnection` puro) em `tests/remote_node_provider.rs`, 6 testes novos — 33 no total pra `warden-server`, `clippy --all-targets` limpo. **Escopo confirmado com o usuário antes de codar**: só isso nesta rodada — o agente-de-nó de verdade (processo real numa segunda máquina) e a ligação em `build_storage_provider`/`FileConfig` (bloqueada por `build_storage_provider` ser síncrona hoje, enquanto `RemoteNodeProvider::connect` precisa ser assíncrono) ficam pra depois. Ver `PENDING.md` P61/`PHASE.md` Fase 9.4 |
| Ciclo de dependência entre `warden-bootstrap` e `warden-server` (Sessão 59, continuação 6) — **removido na Sessão 105** (P61: a memória fica sempre local e só sincroniza) | Ligar `RemoteNodeProvider` no `build_storage_provider` exige `warden-bootstrap` depender de onde `RemoteNodeProvider` mora — mas `warden-server` **já** depende de `warden-bootstrap` (o hub precisa de `bootstrap()`/`Orchestrator`/`handle_turn`). Depender de volta seria um ciclo, impossível no Cargo. Duas opções levadas ao usuário: separar `warden-server` em dois crates (protocolo/cliente vs. hub) vs. só implementar o agente-de-nó real primeiro e adiar essa ligação — **decisão tomada via plano escrito e aprovado** (`EnterPlanMode`/`ExitPlanMode`, prática pedida pelo usuário a partir desta sessão) | **Separar em `crates/warden-server-protocol`** ✓ — `protocol.rs`/`client.rs`/`remote_node.rs` movidos verbatim (`git mv`, sem mudança de conteúdo) pro crate novo, que só depende de `warden-core` (nenhum dos três tinha dependência de `warden-bootstrap` pra começo de conversa — só `server.rs`/`main.rs` do hub tinham). `remote_tool.rs` (`RemoteTool`/`RemoteToolChannel`, Fase 7.4) **ficou** em `warden-server` — só o hub usa, `warden-bootstrap` nunca precisaria. `warden-server` (hub, mais magro agora) reexporta tudo do crate novo (`pub use warden_server_protocol::{ClientMessage, RemoteNodeProvider, ServerConnection, ServerMessage};`) — nenhum dos 5 arquivos de teste existentes precisou mudar import, todos continuam resolvendo via `warden_server::{...}`. `warden-bootstrap` ganhou a dependência do crate novo (sem ciclo, já que ele não depende de nada de volta). `build_storage_provider` virou `async fn` (único jeito de construir um `RemoteNodeProvider` de verdade — precisa de uma conexão de rede real pra existir) com um terceiro parâmetro `remote_node: Option<&RemoteNodeConfig>` (struct nova: `server_url`/`device_id`/`device_name`/`auth_key`/`target_device_id`, em `FileConfig.remote_node`, config.toml/env-only). `desktop`'s `save_settings` só precisou de `.await` nos dois pontos que já chamavam a função (fluxo de migração) — **sem mudar a UI**: `remote_node` continua "Coming soon"/rejeitado por lá, decisão explícita (não tem como preencher `RemoteNodeConfig` sem uma tela que ainda não existe). `cargo check`/`clippy --workspace --all-targets` limpos; `warden-server-protocol` (10 testes, movidos verbatim) + `warden-server` (23) + `warden-bootstrap` (43, 3 novos pro `build_storage_provider`) todos passando. **Ainda em aberto**: o agente-de-nó real do lado alvo — sem ele, `[remote_node]` não tem com quem falar de verdade — ver `PENDING.md` P61 |
| `warden-node` — agente-de-nó real, lado alvo do `RemoteNodeProvider` (Sessão 59, continuação 7) — **removido na Sessão 105** (P61: a memória fica sempre local e só sincroniza) | Fecha a última peça grande do P61 v2: um processo de verdade que serviria `vault_read`/`write`/`list`/`delete` numa máquina remota. Plano escrito e aprovado antes de codar | **Binário novo `crates/warden-server/src/bin/warden-node.rs`**, lógica em `vault_node.rs` — mesmo split `server.rs`/`main.rs` já usado pro hub. `serve()` conecta como cliente (`ServerConnection::connect_with_tools`, Fase 7.4) anunciando as 4 tools, despacha cada `ToolCallRequest` por nome direto pra um `LocalFSProvider` — **zero código de I/O novo**, só a marshalling JSON/base64 que já era o contrato documentado pelo `RemoteNodeProvider` (mesmas 4 chaves, mesmo formato). CLI mesmo shape de `main.rs` (`--server-url`/`--device-id`/`--auth-key` com fallback `WARDEN_SERVER_AUTH_KEY`/`--vault-path`/`--config`, resolução via `warden_bootstrap::{load_config, resolve_vault_path}`). **Verificação de ponta a ponta de verdade** (`tests/vault_node_end_to_end.rs`) — primeira vez que o `RemoteNodeProvider` fala com um alvo real em vez de um roteirizado: `Server` real + `vault_node::serve` real contra um `Vault` num diretório temporário real + `RemoteNodeProvider` real do outro lado, com cada asserção conferindo o arquivo de verdade em disco (não só o round-trip do RPC) — write cria o arquivo, delete apaga de verdade. `cargo test -p warden-server` (25, 2 novos), `clippy --all-targets` limpo, `cargo run --bin warden-node -- --help` confirma o binário. `PHASE.md` Fase 9.5 marcada `[x]` (escopada às 4 tools de vault). **Fora de escopo, decisão explícita**: nenhuma história de deploy/systemd/empacotamento pro `warden-node` rodar numa máquina remota de verdade — só o binário existindo e funcionando, mesma postura "maquinário aditivo" do resto do P61 |
| Pareamento persistente de dispositivo (Fase 9.3, Sessão 60, continuação) | Hoje qualquer dispositivo que sabe o `auth_key` compartilhado é imediatamente um alvo/chamador de roteamento válido (`CallDeviceTool`), sem noção de "este dispositivo específico foi autorizado" — buraco real de segurança que 9.6 (workspace)/9.7 (QR) não fariam sentido sem antes fechar. Opções: aprovação obrigatória em TODA conexão (`Hello` passa a poder recusar/pausar) vs. aprovação só na superfície que realmente expõe o vault de um nó pro outro (`CallDeviceTool`) | **Aprovação só no roteamento** ✓ (confirmado no plano aprovado antes de codar) — `Hello`/`Chat`/`Ping` continuam funcionando pra qualquer dispositivo com o `auth_key` certo, sem quebrar os testes de handshake/chat existentes nem a UX de "conectar e já poder conversar". `crates/warden-server/src/device_registry.rs`: `PairingStore` novo, JSON persistido (`~/.config/warden/devices.json`, `default_server_devices_path` em `warden-bootstrap`) com `Pending`/`Approved`/`Revoked` por `device_id` — **deliberadamente sem cache em memória**, cada método relê o arquivo do disco antes de mutar/responder, porque o `warden-server` (processo longo) e um `warden-server devices approve <id>` (processo separado, one-shot) só têm esse arquivo como coordenação; uma aprovação feita com o servidor já de pé precisa valer na *próxima* `CallDeviceTool` sem reiniciar nada. `server.rs`: todo `Hello` bem-sucedido chama `record_seen` (silencioso — dispositivo novo vira `Pending`, mas ainda recebe `HelloAck` normal); `CallDeviceTool` passou a checar `Approved` tanto do chamador quanto do alvo antes de rotear — **"não conectado" (alvo nunca viu um `Hello`) vence sobre "não aprovado"** de propósito: um `approve` exige que o dispositivo já exista no registro (`record_seen` já rodou), então um id que nunca conectou não tem como ser aprovado — liderar com "não aprovado" mandaria o operador atrás de algo estruturalmente impossível. Revogar não força-desconecta uma sessão já aberta; a checagem por chamada já basta pra bloquear a próxima tentativa de roteamento. `main.rs` virou subcomandos `clap` (`serve` com as mesmas flags de sempre; `devices list/approve/revoke`, que não chamam `bootstrap()` — não precisam de API key configurada, só abrem o `PairingStore`) — sem histórico de deploy real ainda pra esse binário (`PENDING.md` P61), mudar o formato de invocação não quebra nada em produção. 9 testes novos/ajustados em `device_registry.rs`+`device_routing.rs`+`remote_node_provider.rs`+`vault_node_end_to_end.rs` (persistência sobrevivendo a "restart", os 3 casos do gate: chamador não aprovado, alvo não aprovado mas conectado, dispositivo revogado) — `cargo test -p warden-server` (43, era 34) e `clippy --workspace --all-targets` limpos. Verificado manualmente via CLI (sem precisar de API key): `devices list` vazio, aprovar/revogar um id nunca visto erra, e o ciclo completo list→approve→list→revoke→list contra um `devices.json` simulado. **Ainda em aberto**: 9.6 (workspace com UI pra aprovar/revogar visualmente, em vez de CLI) e 9.7 (pareamento via QR) — ambos agora têm uma fonte de verdade persistida pra se apoiar. Ver `PENDING.md` P61/`PHASE.md` Fase 9.3 |
| Topologia da 9.6 — workspace de máquinas no desktop | UI só faz sentido se souber falar com o `PairingStore` de verdade. Opções levadas ao usuário: assumir desktop+`warden-server` na mesma máquina (lê/escreve `devices.json` local direto, nova dependência no crate `warden-server`) vs. superfície admin nova no protocolo WS (`ClientMessage::ListDevices`/`ApproveDevice`/`RevokeDevice`, cobre hub remoto mas levanta uma questão de confiança nova — quem pode aprovar, com que credencial — sem resposta em nenhum lugar do código hoje) | **Mesma máquina** ✓ — fatia bem menor, sem mexer no protocolo de rede nem abrir a questão de confiança em aberto; cobre o caso de uso real de hoje (nenhuma infra de deploy multi-máquina existe ainda, P61). `desktop/src-tauri` ganhou dependência direta em `warden-server` (antes só `warden-bootstrap`/`warden-core`/`warden-sync`) — `workspace_cmds.rs` novo (mesmo padrão de módulo próprio que `vault_cmds.rs`/`sync_cmds.rs`), sem `AppState` novo já que `PairingStore` é stateless por design. 3 comandos (`list_paired_devices`/`approve_paired_device`/`revoke_paired_device`) chamam `PairingStore::new(default_server_devices_path())` fresco a cada chamada — o mesmo arquivo que a CLI `warden-server devices` já lê/escreve, então uma aprovação feita por qualquer um dos dois lados aparece pro outro sem nada especial. `PairedDeviceInfo` (DTO local, `#[serde(rename_all = "camelCase")]`) separa o formato de IPC do formato do arquivo em disco (`PairedDevice`/`PairingStatus` do `warden-server`, que ficam em `snake_case` — mesma separação já usada por `RemoteNodeConfigPayload`/`RemoteNodeConfig`). `WorkspaceView.tsx` novo (mesmo esqueleto de `UsageView.tsx`) — lista com badge de status (reaproveita o visual de `.storage-provider-badge`) e botão de ação contextual (`Approve`/`Revoke`), com um aviso didático explícito de que só enxerga o hub desta máquina. 1 teste novo (`workspace_cmds::tests`) trava o contrato JSON exato (camelCase) que o frontend espera — primeiro teste Rust do crate `desktop` (não existia nenhum antes; os comandos de `vault_cmds.rs`/`sync_cmds.rs` nunca tiveram, por precisarem de `AppState`/Tauri de verdade — este pôde ser isolado porque a função de mapeamento (`to_info`) é pura, sem tocar disco). `cargo test/clippy -p desktop`, `npx tsc --noEmit`, `npm run build` limpos. **Não verificado com Chrome/Playwright real** — extensão não conectada neste ambiente, mesma lacuna já registrada em sessões anteriores. Ver `PENDING.md` P61/`PHASE.md` Fase 9.6 |
| Papel do Warden no ecossistema e escopo da integração MCP (P13, Sessão 95) | Cada produto do ecossistema com o próprio chat de IA vs. Warden como interface conversacional única; TruthID dentro ou fora da integração MCP | **Warden é a única interface conversacional do ecossistema** ✓ (decisão do usuário, 2026-09-23) — o Anchor removeu o próprio AI chat panel por isso; os outros produtos expõem capacidades via MCP e a conversa acontece no Warden. Integração MCP com **Anchor e Lume** ✓. **TruthID fora da integração MCP** ✓ — pouca utilidade e considerado perigoso expor identidade/autenticação como tools do agente. Não mexe nos usos não-MCP do TruthID (pagador Arweave via `pin()`, login da Fase 10) |

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

- **Invocação leve (v1)**: agente principal chama sub-agente escopado pra tarefa específica, contexto reduzido, devolve resultado, encerra. Implementado como `DelegateTool` (`crates/warden-core/src/tool/delegate.rs`, tool `delegate_task`) — internamente é só mais um `Orchestrator` completo (mesmo `model`, mesmo `vault`, subconjunto de tools escolhido pelo chamador), reaproveitando 100% do loop de tool-calling existente em vez de duplicar lógica
- **Delegação recursiva com profundidade limitada (P46, núcleo — Sessão 57)**: primeiro pedaço de "sub-agentes autônomos" a sair do papel. Escopo confirmado com o usuário antes de codar — só o mecanismo de recursão em si (agentes podem criar outros agentes, não só um orquestrador raiz fixo), **sem** fila de jobs, controle de custo ou isolamento de tools por sub-agente, que seguem fora de escopo (ver P46 em `PENDING.md`, e P60 abaixo pro risco aceito). A trava original ("o orchestrator passado pro `DelegateTool` nunca pode ter outro `DelegateTool` registrado") foi removida — `DelegateTool` agora suporta uma cadeia de qualquer profundidade, desde que quem a monte pare de registrar `delegate_task` em algum nível. `warden_bootstrap::build_delegating_orchestrator` (novo, recursivo) monta essa cadeia: registra as mesmas `base_tools` em cada nível, e enquanto `depth > 0` registra também um `DelegateTool` apontando pra outro orchestrator montado um nível mais raso, até `depth == 0` (terminal, sem `DelegateTool`). **O critério de parada é estrutural, não uma checagem em runtime**: o orchestrator terminal simplesmente nunca anuncia `delegate_task` no `tool_specs` mandado pro modelo — não existe um "if depth <= 0, recusar a chamada", o próprio modelo nunca vê a tool naquele nível pra tentar chamá-la. `DEFAULT_DELEGATE_MAX_DEPTH = 2`: raiz pode delegar (nível 1), o sub-agente do nível 1 ainda pode delegar de novo (nível 2, terminal). Número pequeno deliberado — sem fila de jobs/controle de custo, o pior caso é `MAX_TOOL_ITERATIONS ^ depth` chamadas de modelo se **toda** iteração em **todo** nível delegar (8² = 64 nesta profundidade; escalaria rápido com um `depth` maior). **Atualizado (mesma Sessão 57, continuação)**: profundidade virou configurável — `FileConfig.delegate_max_depth: Option<u32>` (`config.toml`) com `WARDEN_DELEGATE_MAX_DEPTH` vencendo por cima (`resolve_delegate_max_depth`, mesmo padrão env-vence-arquivo de `resolve_flag`/`enable_shell`), caindo pro default `2` quando nenhum dos dois está setado. Deliberadamente **sem UI** (Settings do desktop carrega o valor existente adiante sem expor campo — `existing.delegate_max_depth`, mesmo tratamento já dado a `telegram_bot_token`) e **sem teto/clamp** — aumentar o valor é o próprio usuário aceitando conscientemente um `MAX_TOOL_ITERATIONS ^ depth` maior, documentado no doc-comment da constante e em P60, não uma validação que bloqueie. Verificado só com um teste unitário determinístico (`tool::delegate::tests::supports_bounded_recursive_delegation`, `crates/warden-core/src/tool/delegate.rs`) — uma cadeia de 3 orchestrators (raiz→nível-1→folha) compartilhando um único `ModelProvider` mockado, roteirizado por ordem de chamada (determinístico porque cada `chat_stream` bloqueia em qualquer delegação aninhada antes da próxima chamada acontecer, o mesmo padrão de wiring real do `warden-bootstrap`) — confirma que o nível 1 recebe `delegate_task` de verdade (recursão genuína, não só um nível) e que a folha nunca recebe a tool (prova do critério de parada estrutural). **Sem teste de ponta a ponta com um modelo real** — forçar um modelo de verdade a decidir delegar duas vezes seguidas de propósito não é confiável de scriptar, mesma limitação que a primeira versão do `DelegateTool` (Sessão 11) já tinha aceitado (só testes mockados, nunca uma chamada real decidindo delegar). `cargo test -p warden-core -p warden-bootstrap` (65 testes unitários no `warden-core` + as suítes de integração existentes, 35 no `warden-bootstrap`, nenhum quebrado) e `cargo clippy --workspace --all-targets` limpos
- **`delegate_to_agent` — chefe delega pra um agente configurado específico (P46, Sessão 57, continuação)**: mecanismo concreto do "modo centralizado" — diferente de `delegate_task` (sub-agente anônimo, sem persona própria), essa tool endereça um agente **configurado** (persona/provider do `config.toml`) por id. Confirmado com o usuário: opt-in por agente (`AgentConfig.can_delegate_to_agents: bool`, `#[serde(default)]` pra retrocompatibilidade) — só quem tem a flag ligada ganha a tool, o resto continua "funcionário" isolado. **Descoberta que mudou o design**: existe um único `Orchestrator` compartilhado por todas as conversas (tools fixas desde `bootstrap()`); cada canal só troca persona/modelo por cima a cada turno, nunca o conjunto de tools. Por isso a tool não podia ser registrada em `bootstrap()` (daria pra qualquer conversa, não só chefes) — o opt-in de verdade exigiu `Orchestrator::with_tool` novo (mesmo padrão de `with_model`: clona e registra uma tool a mais) e que **cada canal que já resolve `agent_id` por turno** (desktop `send_message`, CLI `resolve_turn_context`/`run_turn` — Telegram/WhatsApp/`warden-server` não têm suporte a agente nomeado nenhum) confira a flag e anexe a tool condicionalmente, turno a turno. `DelegateToAgentTool`/`NamedSubAgent` (`crates/warden-core/src/tool/delegate_to_agent.rs`) continuam `warden-core` agnóstico de `AgentConfig` — recebem de fora uma lista já resolvida (id, descrição/persona, `Orchestrator` já com `with_model` aplicado se `provider_id` difere). `warden_bootstrap::build_delegate_to_agent_tool(config, orchestrator)` novo monta essa lista a partir de `config.agents`, reaproveitando o mesmo `orchestrator` do turno como base de cada alvo (herda base_tools/profundidade de delegação de graça) — pula (com aviso em `eprintln!`, não fatal) um agente cujo `provider_id` não resolve. **Limitação aceita e deliberada**: um agente invocado como *alvo* nunca ganha `delegate_to_agent` ele mesmo, mesmo que o próprio `can_delegate_to_agents` seja `true` — essa flag só é consultada pelo canal pro agente *ativo* da conversa, nunca pra um alvo de delegação. Evita cadeia chefe-de-chefe descontrolada sem precisar de outro limite de profundidade, mesmo espírito de P60. **Sem UI/CLI ainda pra ligar a flag** — só hand-edit do `config.toml` (mesma decisão de escopo do `delegate_max_depth`); os 4 pontos que constroem `AgentConfig` literalmente (fixture/helper de teste do `warden-bootstrap`, `desktop::save_settings`, CLI `wizard_agents_create`/`wizard_agents_edit`) foram ajustados pra carregar o valor existente adiante em vez de resetar pra `false` a cada save (mesmo cuidado de `telegram_bot_token`/`delegate_max_depth`) — exigiu mover o carregamento de `existing` pro topo de `save_settings`, antes só acontecia depois do loop de agentes. 9 testes novos em `delegate_to_agent.rs` (falta de argumento, despacho por id, erro em id desconhecido, persona chega de verdade como mensagem de sistema, spec lista todo agente). `cargo test -p warden-core -p warden-bootstrap -p warden-cli` (70+36+34+5 testes) e `cargo clippy --workspace --all-targets`/`cargo check --workspace` limpos; `npm run build` (tsc+vite) do desktop limpo (zero mudança de TS esperada). **Sem teste de ponta a ponta com modelo real** — mesma limitação já aceita pro núcleo recursivo (P60): forçar um modelo real a escolher `delegate_to_agent` de propósito não é confiável de scriptar
- **UI/CLI pra ligar `can_delegate_to_agents` (P46, Sessão 57, continuação 8)**: fecha a lacuna deixada pela entrada anterior — até aqui a flag só existia via hand-edit do `config.toml`. Desktop: `AgentPayload` (`desktop/src-tauri/src/lib.rs`) e `AgentEntry` (`desktop/src/types.ts`) ganharam `can_delegate_to_agents`/`canDelegateToAgents`; `AgentCard` (`SettingsView.tsx`) ganhou um checkbox "Can delegate to other agents", mesmo padrão visual (`settings-checkbox-field`/`settings-checkbox-row`) já usado pelo checkbox de OAuth do MCP. **Isso inverteu a decisão anterior de "carregar o valor existente adiante"**: `save_settings` parou de sempre puxar `existing.agents.find(...).can_delegate_to_agents` (o "carry forward" que a entrada acima descreve) e passou a usar o valor que vem no `AgentPayload` do form — sem esse checkbox no form, esse carry-forward era o único jeito de a flag não resetar a cada save; com o checkbox, ele vira a própria fonte da verdade, exatamente como `id`/`persona`/`provider_id` já eram. CLI: `prompt_agent_can_delegate` novo (`crates/warden-cli/src/interactive.rs`) — prompt s/n reaproveitado por `wizard_agents_create` (default "n") e `wizard_agents_edit` (default = valor atual do agente); `cmd_agents_list` (`/agents`) ganhou o marcador `[delega]` ao lado de quem tem a flag ligada, ao lado do `[ativo]` já existente. `cargo test -p warden-core -p warden-bootstrap -p warden-cli` e `cargo clippy --workspace --all-targets` limpos; `tsc`/`npm run build` do desktop limpo. **Verificado via Playwright headless contra o dev server real** (`get_settings`/`save_settings` mockados via `addInitScript`, mesmo padrão da tela de Usage — Sessão 49): checkbox localizado e marcável, payload de save confirmado com `canDelegateToAgents: true`, zero erro de console

---

## Comandos de barra no `warden-cli` (`/models`, `/agents`) — Sessão 49

Trazem pro terminal a mesma gestão de provedores/agentes que o desktop só tinha
na Settings, escrevendo no mesmo `config.toml` — decisões que valem registrar:

- **Estado de seleção é por sessão do processo, não persistido** — dois
  `Option<String>` (`CliSession.provider_id`/`agent_id`) dentro de
  `interactive.rs`, nunca guardando um `Arc<dyn ModelProvider>` ou persona
  resolvidos. Antes de cada turno que tem alguma seleção ativa, o config é
  **relido do disco na hora** (`resolve_turn_context`) — mesmo padrão que o
  `send_message` do desktop já usava pra nunca cachear um objeto resolvido
  entre chamadas, evitando ficar com uma referência obsoleta depois de um
  rename/delete no meio da sessão. Quando nenhuma seleção está ativa, zero
  leitura de disco extra por turno — comportamento idêntico ao de antes dessa
  feature existir.
- **Cascade de rename/delete de provider** (mesmas regras já validadas no
  desktop, P32/P33) **não existia em Rust** — só no `SettingsView.tsx` do
  frontend, que edita um rascunho local e só persiste no Save. Como o CLI
  comita cada comando direto no disco (sem esse rascunho), a cascade virou
  código de verdade em `warden-bootstrap` (`rename_provider_cascade`,
  `remove_provider_references`) — puro, testável sem terminal, atualiza
  `active_provider` e todo `agents[].provider_id` que apontava pro id
  velho/removido. O CLI soma a isso limpar seu próprio estado de sessão em
  memória quando apontava pro id afetado (o desktop não tem esse conceito).
- **Wizard de múltiplos campos reaproveita o `LineEditor` existente**, não um
  crate de formulário novo — `read_line`/`render_input_box` foram só
  parametrizados (`drive_line_editor` extraído do loop de teclas, título da
  caixa virou argumento) e ganharam um `read_field(title, initial)` que
  pré-preenche o buffer (Enter sem editar aceita o valor atual/default,
  `Ctrl+D` em campo vazio cancela o wizard inteiro).
- **Limitações aceitas deliberadamente**: persona de agente é uma linha só
  (sem textarea no editor hand-rolled); chave de API digitada no wizard não é
  mascarada (some da tela no próximo `clear()`, nunca vai pro arquivo de
  histórico); só o REPL interativo ganhou os comandos — `run_plain` (stdin via
  pipe, só usado por teste) não.

---

## Setup Tauri Mobile (Fase 7.1) — Sessão 49

Primeira etapa da Fase 7. Feito só pro lado **Android**; **iOS não tem como ser
validado neste container** — exige Xcode, que só roda em macOS. A config
`tauri.conf.json`/`Cargo.toml` já é genérica o bastante pra cobrir os dois
(mesmo `crate-type = ["staticlib", "cdylib", "rlib"]` e
`#[cfg_attr(mobile, tauri::mobile_entry_point)]` que o scaffold original do
`create-tauri-app` já deixou prontos desde o início do projeto), mas o lado
iOS fica sem nenhum teste até rodar num Mac de verdade.

- **Toolchain Android instalado sem tocar no Rust do sistema (pacman) nem
  pedir `sudo`** — o ambiente não tinha JDK/Android SDK/NDK, e o `rustc` do
  sistema (via pacman) não tem os targets de cross-compile que `rustup`
  gerencia. Em vez de arriscar mexer no Rust do sistema (usado por todo o
  resto do workspace) ou depender de `sudo` (que pede senha, indisponível pro
  agente), tudo foi instalado **sem privilégio de root, isolado em
  `~/.local/opt/`**: JDK 17 (Temurin, tarball direto), Android cmdline-tools +
  SDK (platform 34/36, build-tools, NDK 27, emulator, imagem de sistema
  x86_64) via `sdkmanager`, e um `rustup` **paralelo** só para os 4 targets
  Android (`aarch64`/`armv7`/`i686`/`x86_64-linux-android`) — instalado com
  `--no-modify-path` de propósito, então o `cargo`/`rustc` que todo o resto do
  projeto usa no dia a dia continua sendo o do pacman, sem mudar de versão por
  baixo dos panos. Builds mobile precisam exportar `JAVA_HOME`/`ANDROID_HOME`/
  `PATH` manualmente na hora (não persistido em nenhum shell rc) — replicável
  em outra máquina/sessão seguindo os mesmos passos, mas vale considerar um
  script `setup-android-toolchain.sh` se isso for repetido com frequência.
- **`minSdkVersion` subiu de 24 (default do template) pra 26 em
  `tauri.conf.json`** (`bundle.android.minSdkVersion`) — o primeiro build
  falhou no link (`ld.lld: error: unable to find library -laaudio`): o
  `cpal` (gravação nativa de voz, P28/Sessão 41) linka contra `libaaudio.so`
  incondicionalmente no target Android, mas essa lib só existe no sysroot do
  NDK a partir da API 26 (AAudio foi introduzida no Android 8.0). Android
  8.0+ já cobre a esmagadora maioria dos devices ativos em 2026, então subir o
  mínimo foi a correção certa (não um workaround) — nenhuma feature foi
  cortada. Fonte de verdade é o `tauri.conf.json`, não o `build.gradle.kts`
  gerado em `gen/android/`, que é sobrescrito a cada `tauri android init`.
- **Verificado de ponta a ponta com emulador de verdade, não só build**:
  `cargo tauri android build --debug --apk` compilou `desktop_lib` pros
  targets `aarch64` e depois `x86_64` (o segundo, específico pra rodar
  acelerado via KVM — `/dev/kvm` disponível neste container); AVD
  `warden_test` (`system-images;android-34;google_apis;x86_64`) criado via
  `avdmanager`, emulador subido headless (`-no-window -gpu
  swiftshader_indirect`), boot completo em ~68s, APK instalado via `adb
  install`, app aberto via `adb shell monkey -p com.warden.desktop`, e
  **screenshot real via `adb exec-out screencap`** confirmando que a UI do
  React (a mesma do desktop, sem nenhuma mudança) renderiza dentro do
  WebView do Android.
- **Achado real do teste (não um bug — vira trabalho da 7.3)**: a sidebar de
  largura fixa (`280px`, grid `280px 1fr` em `App.css`) praticamente toma a
  tela inteira num celular (a AVD usa 320×640 lógicos) — a área de chat
  sobra como uma faixa de ~40px. Confirma que a 7.3 ("Interface de chat
  mobile") precisa mesmo de um layout responsivo dedicado, não é só
  "reaproveitar a UI do desktop sem mexer".
- **`desktop/src-tauri/gen/android/` commitado** (exceto `build/`,
  `.gradle/`, `local.properties` — já cobertos pelo `.gitignore` que o
  próprio `tauri android init` gera dentro da pasta) — é pequeno sem os
  artefatos de build (~620KB, 55 arquivos: `AndroidManifest.xml`,
  `build.gradle.kts`, wrapper do Gradle) e é onde customização nativa
  específica do Android (ícones, permissões, etc.) vai morar quando a 7.3
  precisar. Convenção oficial do Tauri — evita todo mundo que mexer no
  projeto ter que rodar `tauri android init` de novo do zero.
- **`cargo clean` rodado no meio da sessão** (liberou 76GB de um `target/`
  que tinha crescido acumulando builds de sessões anteriores, chegando a 96%
  de disco ocupado na máquina real do usuário) — confirmado com o usuário
  antes de rodar, não é automático.

## Mobile: troca de Tauri Mobile pra Flutter (Sessão 50, continuação)

**Decisão revertida** — a escolha original de Tauri Mobile pro app mobile
(linha "Framework mobile" no topo deste arquivo) foi trocada por **Flutter**,
a pedido explícito do usuário: prioriza **maturidade geral** e **suporte a
iOS** acima do reuso de código que motivou a escolha original do Tauri.

**Por que o timing é bom pra reverter agora**: só a 7.1 (setup do toolchain
Android + confirmação de que a UI do desktop renderiza dentro do WebView, ver
seção acima) tinha sido feita. Nenhuma etapa de feature (7.2 a 7.6) foi
implementada — é o ponto mais barato possível da Fase 7 pra trocar de stack.

**Por que a troca não afeta nada já decidido pro backend**: o `PHASE.md`
(Fase 7) já define o mobile como **cliente puro** ("nunca servidor"),
conversando com o servidor via Tailscale + WebSocket usando o protocolo JSON
próprio decidido na linha "Protocolo servidor↔cliente" (P1, mesma Sessão 50)
— uma decisão explicitamente agnóstica de linguagem/framework. O mobile nunca
ia embutir o `warden-core` em Rust diretamente (isso exigiria FFI/bindings
tipo `flutter_rust_bridge` em qualquer framework não-Rust); ia só falar
JSON por WebSocket com o servidor, exatamente o que um app Flutter faz
normalmente. Ou seja: trocar o client de UI não reabre nem o `warden-server`
nem o schema de mensagens já fechados.

**O que se perde**: o reuso "de graça" da UI React do desktop, motivo
original da escolha de Tauri (ver linha "Framework desktop" no topo). Mas a
própria 7.1 já tinha achado que esse reuso valia menos do que parecia — a UI
fixa do desktop (sidebar de 280px) não serve como está numa tela de celular
(ver achado registrado acima e P35 em `PENDING.md`), então a 7.3 já ia exigir
um layout mobile dedicado de qualquer forma, com ou sem Tauri.

**O que se ganha, alinhado com o pedido do usuário**: suporte a iOS maduro em
produção há anos (diferente do Tauri Mobile, cujo lado iOS nem chegou a ser
testado nesta sessão por falta de Xcode/macOS — ver seção acima, e isso não
muda de framework nenhum, mas o ecossistema/comunidade em volta do iOS no
Flutter é ordens de grandeza mais fundo); e um ecossistema de plugins bem
mais maduro pra 7.4 (arquivos/permissões) e principalmente 7.5 (push
notification via FCM/APNs), onde o Tauri Mobile ainda é bem mais cru.

**Trabalho que essa reversão implica** (registrado como prioridade alta em
`PENDING.md` P35): reverter/depreciar o scaffold Android do Tauri Mobile
(`desktop/src-tauri/gen/android/`, toolchain em `~/.local/opt/`) e recomeçar
a 7.1 num projeto Flutter novo — provavelmente fora de `desktop/` (não é
mais uma extensão do mesmo app Tauri, é um client separado). `PHASE.md`
(Fase 7) atualizado pra refletir a stack nova.

## 7.1 refeita em Flutter (Sessão 50, continuação)

**Scaffold Tauri Mobile removido de vez**: `desktop/src-tauri/gen/android/`
(commitado na Sessão 49) apagado via `git rm`, junto do bloco
`bundle.android.minSdkVersion` em `tauri.conf.json` (a única parte de fato
mobile-específica dessa config — `crate-type`/`mobile_entry_point` no
`Cargo.toml`/`main.rs` do `desktop/src-tauri` já eram scaffold genérico do
`create-tauri-app` desde o início do projeto, não removidos). O restante de
`gen/` (`schemas/*.json`) nunca foi versionado — é build output regenerado
pelo próprio Tauri, apagado do disco por limpeza mas sem efeito no git.

**Toolchain Android de `~/.local/opt/` reaproveitado, não descartado** —
diferente do scaffold em si, o JDK 17/Android SDK/NDK instalados sem sudo na
Sessão 49 (com as licenças já aceitas) servem exatamente do mesmo jeito pro
Flutter, que também precisa desse toolchain pra compilar Android. `flutter
config --android-sdk ~/.local/opt/android-sdk` + `JAVA_HOME` apontado pro
mesmo JDK bastou — `flutter doctor` confirmou "All Android licenses
accepted" sem precisar aceitar nada de novo.

**Flutter SDK instalado sem sudo, mesmo padrão da Sessão 49** — clone raso
(`git clone --depth 1 -b stable`) direto pra `~/.local/opt/flutter`, sem
tocar em pacman/apt. Único obstáculo: o ambiente não tem `unzip` instalado
(usado internamente por `update_dart_sdk.sh` pra extrair o Dart SDK) e
instalar via pacman pediria sudo. Em vez de arriscar mexer no sistema,
criado um shim (`~/.local/bin/unzip`, à frente no `PATH`) que traduz a
chamada específica que o Flutter faz (`unzip -o -q FILE -d DIR`) pra
`bsdtar -x -f FILE -C DIR` — `bsdtar` (libarchive) já vinha instalado no
sistema e lê zip nativamente. Resolve só o caso de uso real do Flutter, não
é um `unzip` completo.

**Projeto novo em `mobile/`, raiz do repo** — `flutter create --org
com.warden --project-name mobile --platforms android,ios mobile`, gerando
`applicationId com.warden.mobile` (paralelo ao `com.warden.desktop` do lado
Tauri). Fora de `desktop/` de propósito: não é mais uma extensão do mesmo
app, é um client de UI totalmente separado que só fala WebSocket/JSON com o
`warden-server` (P1) — nenhuma dependência Rust/FFI embutida.

**Verificado de ponta a ponta, mesmo rigor da checagem anterior em Tauri
Mobile**: `flutter build apk --debug` rodou o pipeline completo (Gradle,
primeira execução, baixou sozinho o NDK r28c e o Build-Tools 36 que
faltavam pro target novo — nenhum dos dois precisou de intervenção manual),
gerando um APK real (~150MB, debug). Instalado no mesmo AVD `warden_test`
já existente da Sessão 49 (`adb install -r`), aberto via `adb shell monkey`,
e **screenshot real via `adb exec-out screencap`** confirmando a tela padrão
do Flutter (contador "You have pushed the button this many times") — prova
que a cadeia inteira (Dart → Gradle/Kotlin/aapt2 → APK → instalação →
runtime Android) funciona neste ambiente, do jeito que também foi provado
pro Tauri Mobile antes de reverter. `mobile/.gitignore` (gerado pelo próprio
`flutter create`) já cobre `/build/`, `.dart_tool/`,
`android/local.properties`, `android/.gradle`, `*.keystore` — nada disso
versionado, confirmado com `git status`.

**iOS segue sem nenhum teste** — `flutter create` já gera o projeto Xcode
(`mobile/ios/`) mesmo sem poder buildá-lo aqui (exige Xcode/macOS,
indisponível neste container Linux). Mesma limitação exata que já existia
com Tauri Mobile antes da troca — não é uma lacuna nova introduzida pelo
Flutter, é a mesma lacuna de ambiente carregada adiante. Registrado como
`PENDING.md` P39.

**Disco**: instalação consumiu ~16GB novos entre `~/.local/opt/flutter`
(1.5GB), `~/.gradle` (5GB, cache de dependências Kotlin/AGP — parcialmente
compartilhado com o que o Tauri Mobile já tinha baixado antes), Android
NDK/Build-Tools novos dentro de `~/.local/opt/android-sdk` (SDK total foi
de ~7GB pra 10GB) e `mobile/build/` (~1.9GB, gitignored). Disco ficou em 26GB
livres (86% usado) — não crítico, mas vale lembrar do `cargo clean`/`flutter
clean` se apertar de novo, mesmo aviso já registrado na Sessão 49.

## 7.2 — Flutter conecta ao `warden-server` (Sessão 50, continuação)

Escopo deliberadamente estreito, conforme `PHASE.md`: só provar
conectividade (handshake, heartbeat, erro de auth), não a UI de chat (7.3).
Dois agentes de exploração (protocolo real do `warden-server`, estado do
scaffold `mobile/`) + um agente de design produziram o plano; implementação
seguiu esse plano quase à risca, com uma correção real encontrada e
corrigida durante a implementação (abaixo).

**Camada de protocolo** — `mobile/lib/protocol/messages.dart`: `sealed
class ClientMessage`/`ServerMessage` (Dart 3.13), uma subclasse `final` por
variante, `toJson`/`fromJson` na mão — espelha `crates/warden-server/src/
protocol.rs` campo a campo (`camelCase`, tag `"type"`). Testado com 9 casos
de round-trip (`test/protocol/messages_test.dart`) comparando string JSON
literal contra o que o lado Rust produz, incluindo o caso de `Goodbye`
serializar `"reason":null` explícito em vez de omitir o campo.

**Serviço de conexão** — `mobile/lib/services/server_connection.dart`:
espelha `ServerConnection` de `client.rs`, escrito contra
`StreamChannel<dynamic>` (não `WebSocketChannel` direto — `WebSocketChannel`
já *é* um `StreamChannel`, confirmado lendo o código-fonte real do pacote).
**Achado real durante a implementação, não previsto no plano**: tanto
`WebSocketChannel.stream` quanto as duas metades de um
`StreamChannelController` são de fato *single-subscription* (confirmado lendo
`adapter_web_socket_channel.dart` e `stream_channel_controller.dart` do
pacote) — um stream desses só pode ser escutado (`.listen()`) uma única vez
na vida. O plano original chamava `channel.stream.first` no handshake e
depois `channel.stream.listen(...)` de novo no modo conectado — teria
lançado `Bad state: Stream has already been listened to` em produção
assim que o primeiro `HelloAck` chegasse. Corrigido criando **uma única**
`StreamSubscription` antes de mandar `Hello`, com um `Completer` resolvendo
a resposta do handshake; no sucesso, os callbacks (`onData`/`onError`/
`onDone`) dessa mesma subscription são trocados (`.onData(...)`, não um novo
`.listen()`) pro modo conectado — Dart permite trocar os handlers de uma
subscription a qualquer momento, mesmo já recebendo eventos. Isso também
aproxima mais fielmente o `recv()` do `client.rs`, que consome do mesmo
stream continuamente, nunca re-escuta.

Heartbeat a cada 30s (`Timer.periodic` + hook `@visibleForTesting
pingNow()` pra testes não dependerem do timer real) — puramente JSON de
aplicação, sem depender de frame WS nativo de ping/pong (o servidor não
inicia nem exige). Nonce sem pong dentro de um intervalo vira
`ConnectionFailure` (sem retry automático — fora de escopo da 7.2,
documentado no próprio doc-comment da classe). `goodbye()` manda `Goodbye` e
trata o fechamento do socket que vem depois como `Disconnected` limpo
(flag `_goodbyeSent`), já que o servidor nunca confirma — só para de ler e
derruba a conexão.

**Configurações** — `mobile/lib/services/connection_settings.dart`
(`shared_preferences`, texto puro — mesma postura de segurança do OAuth MCP
do desktop, P26) + `device_id.dart` (UUID na mão, `Random.secure()`, sem
pacote `uuid`).

**UI** — `mobile/lib/screens/connection_screen.dart`, `StatefulWidget` puro
(sem pacote de state management — uma tela só). Prefill de host
`10.0.2.2` só em `kDebugMode && Platform.isAndroid` e só quando não há valor
salvo — nunca sobrescreve o que o usuário já digitou, nunca aparece em
iOS/release.

**Android/iOS — permissão de rede que faltava** (achado real do agente de
design, confirmado lendo o arquivo): `mobile/android/app/src/main/
AndroidManifest.xml` não tinha `INTERNET` (só existia nos manifests de
debug/profile, que o Flutter usa só pro próprio hot-reload) nem
`usesCleartextTraffic` — sem isso, Android 9+/API 28+ bloqueia `ws://` por
padrão. Corrigido no manifest principal. Adicionada proativamente a exceção
equivalente de App Transport Security no `Info.plist` do lado iOS
(`NSAllowsArbitraryLoads`) — não dá pra verificar nesta sessão (sem
Xcode/macOS, P39), mas evita uma falha silenciosa quando alguém finalmente
buildar lá.

**Pacotes novos**: `web_socket_channel` (oficial dart-lang, puro Dart),
`shared_preferences` (oficial Flutter), `stream_channel` e `meta` como
dependências diretas (não só transitivas, já que `server_connection.dart`
os importa direto — `flutter analyze` pegou isso, `depend_on_referenced_
packages`), `async` como dev-dependency (`StreamQueue` nos testes). Sem
`json_serializable`/`freezed`/`build_runner` — protocolo pequeno e estável,
mesmo raciocínio do `protocol.rs` escrito na mão do lado Rust.

**Verificado de ponta a ponta contra um `warden-server` real** (não
mockado): `cargo run -p warden-server -- --listen 0.0.0.0:7420 --auth-key
test-key` no host, app instalado no mesmo AVD `warden_test`. Três fluxos
confirmados com screenshot real via `adb exec-out screencap` **e** o log do
servidor do outro lado: (1) handshake com chave certa → tela mostra
"Connected to warden-server", log do servidor mostra `Android Device
(<deviceId>) connected from 127.0.0.1:50738`; (2) toque em "Disconnect" →
tela volta a "Disconnected", log do servidor mostra `<deviceId> said
goodbye (Some("user disconnected"))` — confirma que o `reason` chega
intacto; (3) reconectar com chave errada → tela mostra "Error:
authentication rejected: invalid auth key", replicando a mensagem exata que
`client.rs` produziria no lado Rust. `flutter analyze` limpo, `flutter
test` limpo (14 testes: 9 de protocolo + 4 de `ServerConnection` rodando a
lógica real de handshake/heartbeat/goodbye contra um
`StreamChannelController` real, não mocks — mesmo espírito de
`crates/warden-server/tests/handshake.rs` — + 1 smoke test de widget).
`cargo build --workspace` confirmado limpo (nenhum arquivo Rust tocado).

**Sem Tailscale real disponível** neste ambiente de dev — mesma lacuna já
aceita do lado servidor (`PENDING.md` P36), agora estendida ao cliente
mobile: testado só via `10.0.2.2` (emulador → host), não sobre uma tailnet
de verdade.

## 7.3 — Chat real no Flutter, `warden-server` ganha um `Orchestrator` (Sessão 51)

Antes de codar, o usuário escolheu explicitamente entre chat real (servidor
hospeda um `Orchestrator` de verdade) vs. UI de chat só de mentirinha sem
back-end — optou pelo primeiro, reabrindo de propósito a decisão da 9.2 de
"`warden-server` deliberadamente sem `Orchestrator`/superfície de tool
dispatch". Planejado em modo formal (`/plan`) antes de codar, sem
sub-agentes de exploração desta vez — o código relevante (`protocol.rs`/
`server.rs`/`bootstrap()`/`handle_turn`/o client Dart da 7.2) já tinha sido
lido nesta mesma sessão de continuidade.

**Protocolo** (`crates/warden-server/src/protocol.rs`) — `ClientMessage`
ganha `Chat{message}`; `ServerMessage` ganha `ChatResponse{content,
usage: Option<Usage>}` e `ChatError{message}`. `Usage` é
`warden_core::model::Usage`, reaproveitado direto (já tinha `camelCase`
pronto pra isso) — não inventou um tipo próprio. `ChatError` existe pra
uma falha de `handle_turn` (chave de API ausente, rate limit, erro do
provider) virar uma mensagem que o cliente mostra, em vez de simplesmente
fechar a conexão; carrega o texto real do erro (`format!("{err:#}")`), não
uma mensagem genérica — diferente do `warden-telegram`, que degrada pra uma
string fixa porque é um bot público, aqui é uma ferramenta pessoal.

**`warden-server` ganha `warden-core`/`warden-bootstrap` como dependências**
(path deps, mesmo padrão do `warden-telegram`) — reabre de propósito a
decisão da 9.2. `Server::bind` passou a receber `Arc<Orchestrator>` +
`conversations_dir: PathBuf`; `main.rs` chama `bootstrap()` uma vez no
start (mesmas flags `--vault-path`/`--provider`/`--model`/`--config` do
`warden-telegram`, `default_vault_path()` = `~/Warden/vault`). Nova
`warden_bootstrap::default_server_conversations_dir()`
(`~/.config/warden/conversations-server`), mesma forma de
`default_telegram_conversations_dir`/`default_whatsapp_conversations_dir`.
Uma conversa por `device_id` (a partir do `Hello` já validado) — mesmo
padrão do `chat_id` do Telegram/JID do WhatsApp, usando o `handle_turn`
já existente sem mudar nada nele.

**Bug de concorrência achado e corrigido *antes* de rodar qualquer coisa** —
puramente por raciocínio sobre o código existente, não por reprodução
acidental: `handle_connection` lia e tratava um frame por vez num único
loop; um `Chat` dispara `handle_turn`, que pode levar de ~10s a 70s+ (a
própria Sessão 48 já registrou instabilidade/rate limit do Gemini nessa
faixa). Se tratado inline, o loop de leitura trava esse tempo todo — o
`Ping` que o cliente manda a cada 30s (`heartbeatInterval` em
`server_connection.dart`) não seria *lido* até o `Chat` terminar, e o
`pingNow()` do cliente trata um ping sem pong dentro do próximo intervalo
como conexão morta, matando uma conexão perfeitamente saudável só porque a
resposta do modelo demorou. **Corrigido** com um `tokio::sync::mpsc::
unbounded_channel<ServerMessage>` — depois do handshake (que continua
escrevendo direto no `sink`, como antes), uma task de escrita dedicada
assume o `sink` e drena o canal; o loop de leitura mantém um `tx` e nunca
mais bloqueia: `Ping` manda `Pong` na hora pelo canal, `Chat` clona
`orchestrator`/`conversations_dir`/`device_id`/`tx` e roda `handle_turn`
numa task própria (`tokio::spawn`), mandando `ChatResponse`/`ChatError`
pelo canal quando terminar — o loop de leitura já voltou a ler o próximo
frame nesse meio-tempo. `Goodbye`/erro de parse: `drop(tx)` e `.await` na
writer task antes de retornar, pra fechar limpo.

**Testado sem chave de API nenhuma** — `crates/warden-server/tests/
support/mod.rs`: `MockProvider` implementa só `ModelProvider::chat_stream`
(único método obrigatório da trait), reaproveitando `warden_core::model::
response_stream`/`Response`, o mesmo helper que os testes do próprio
`warden-core` usam pra virar uma resposta enlatada num `ChatStream` sem
tocar rede nenhuma. Três testes novos em `tests/chat.rs`: round-trip normal
(`Chat` → `ChatResponse` com o conteúdo esperado); caminho de erro
(`MockProvider::failing()` → `ChatError`, e a conexão continua respondendo
`Ping` depois — não morre); e **o teste que prova o fix de concorrência**:
`MockProvider` com 2s de delay artificial, manda `Chat` e logo em seguida
um `Ping` — afirma que o `Pong` chega dentro de 500ms (bem antes do
`ChatResponse` de 2s), com um `tokio::time::timeout` explícito que falharia
com uma mensagem clara ("Pong took too long — the reader loop was blocked
by the in-flight Chat call") se o bug reaparecesse. `cargo build/test/
clippy --workspace` limpos (7 testes novos no `warden-server`: 3 de chat +
1 novo de handshake pra cobrir um segundo `Hello` na mesma conexão, que já
existia no código mas não tinha teste).

**Flutter** — `mobile/lib/protocol/messages.dart` ganha `ChatMessage`
(`ClientMessage`), uma classe `Usage` pequena (espelha `warden_core::
model::Usage`, campos opcionais quando o provider não reporta) e
`ChatResponseMessage`/`ChatErrorMessage` (`ServerMessage`) — achado real ao
rodar `flutter analyze`: o switch exaustivo de `_handshake` (só trata o
primeiro frame da conexão) também precisou dos dois casos novos, mesmo
sem nenhum sentido prático de um `Chat*` chegar ali (não compila sem, Dart
exige exaustividade total sobre um `sealed class`). `server_connection.dart`
ganha `sendChat(String)` e `Stream<ServerMessage> get chatStream`
(broadcast, só `ChatResponseMessage`/`ChatErrorMessage`) — zero mudança na
máquina de handshake/heartbeat existente.

**Novo `mobile/lib/screens/chat_screen.dart`** — histórico só em memória
pro tempo de vida da tela (o protocolo não tem mensagem de "buscar
histórico" ainda, registrado como P40; o servidor persiste a conversa em
disco, mas o cliente não pede de volta ao reconectar — escopo estreito
deliberado, mesmo espírito da 7.2). Um turno por vez (`_waitingForReply`
desabilita input/envio enquanto espera, mesma postura síncrona do
desktop/CLI). Botão "Disconnect" explícito na `AppBar` — voltar (seta/
botão de sistema) NÃO desliga a conexão, só navega de volta pra
`ConnectionScreen` (que continua viva por baixo, `Navigator.push` em vez
de `pushReplacement`) — permite o usuário checar a tela de conexão sem
perder a sessão. **Achado durante a própria verificação manual**: como a
`ConnectionScreen` só oferece "Connect"/"Disconnect" (nunca "voltar pro
chat" quando já conectada), voltar da `ChatScreen` sem querer desconectar
não tem caminho de volta pela UI — só desconectando e reconectando de
novo. Aceito como lacuna pequena, registrado em `PENDING.md` (P41) em vez
de corrigido nesta sessão (fora do escopo aprovado).

**Verificado de ponta a ponta contra um `warden-server` real com Gemini de
verdade** (não mockado, config real do usuário em `~/.config/warden/
config.toml`): app no mesmo AVD `warden_test`, conectado via `10.0.2.2`,
duas mensagens reais mandadas e respondidas corretamente (bolhas
renderizando texto real do Gemini, "Thinking…" enquanto espera). A
segunda mensagem pedia um poema de 3 frases (resposta demorou o bastante
pra passar de um ciclo de heartbeat de 30s) — confirmado por screenshot **e**
pelo log do servidor que a conexão nunca caiu nem repetiu o handshake
durante a espera, provando o fix de concorrência também no caminho real,
não só no `MockProvider` com delay artificial. `flutter analyze` limpo;
`flutter test` limpo (20 testes: os 14 anteriores + 6 novos — round-trip
de `Chat`/`ChatResponse` com e sem `usage`/`ChatError`, e dois testes de
`ServerConnection.sendChat`/`chatStream` contra um `StreamChannelController`
real).

**Bônus da mesma sessão, fora do escopo original mas pedido pelo usuário
no meio do trabalho**: a sidebar do desktop nunca teve mecanismo de
recolher — confirmado que não existia nenhum estado/CSS/botão de collapse
em lugar nenhum antes de implementar. Escolhido "rail de ícones" (logo +
"+ nova conversa" + Usage + Settings, todos só com ícone, lista de
conversas some por inteiro) sobre "esconder de vez" — mantém acesso rápido
às ações do rodapé mesmo recolhida. `sidebarCollapsed` novo em `App.tsx`,
persistido via `localStorage` (`warden.sidebarCollapsed` — preferência
por-device, não faz sentido no `config.toml` sincronizável). `Sidebar.tsx`
ganha `collapsed`/`onToggleCollapsed`; `ChevronIcon` novo em `Icons.tsx`
(aponta pra esquerda por padrão, `transform: rotate(180deg)` via CSS
quando recolhida). `.app-shell--sidebar-collapsed` estreita a coluna do
grid de 280px pra 64px; textos escondidos por renderização condicional
(`{!collapsed && ...}`), não por CSS, pra não deixar nó morto no DOM só
pra esconder. Verificado com Playwright headless contra o `vite dev`
server real (não harness estático) mockando `window.__TAURI_INTERNALS__.
invoke`, claro e escuro: expandido → recolhido → expandido de novo, sem
erro de console em nenhum estado. `tsc`/`npm run build` limpos.

## 7.4 — Tool local no mobile: roteamento genérico de tool pro cliente certo (Sessão 52)

Escolhido pelo usuário como próximo passo após a 7.3. Confirmado antes de planejar: como o modelo
roda dentro do `Orchestrator` que o `warden-server` hospeda (7.3), não no celular, uma tool "local"
só funciona se o servidor souber pedir pra *aquela conexão específica* rodar algo e devolver o
resultado — exatamente o mecanismo que `PHASE.md` já reservava pra 9.4/9.5. A 7.4 é o primeiro
consumidor concreto disso, escrito de forma genérica (qualquer tool que um cliente anuncie), não
amarrado só ao caso de arquivos do celular. Escopo fechado com o usuário antes de codar: só leitura
(`list_phone_files`/`read_phone_file`, sem escrita — risco maior, adiado), pasta raiz persistida
(escolhida uma vez, sem picker por chamada), Android-only (sem equivalente iOS pra SAF).

**Protocolo** (`crates/warden-server/src/protocol.rs`) — `ClientMessage::Hello` ganha
`#[serde(default)] tools: Vec<warden_core::tool::ToolSpec>` (retrocompatível — um cliente antigo ou
sem nada pra anunciar não manda o campo). `warden_core::tool::ToolSpec` ganhou `Serialize`/
`Deserialize` (`crates/warden-core/src/tool/mod.rs`) pra ser reaproveitado direto como formato de
wire, sem duplicar a struct. Novo `ServerMessage::ToolCallRequest{call_id, tool, arguments}` e
`ClientMessage::ToolCallResult{call_id, result}`/`ToolCallError{call_id, message}` — `call_id` é um
contador simples por conexão, mesmo espírito do `nonce` do `Ping`.

**`crates/warden-server/src/remote_tool.rs`** (novo) — `RemoteTool`, um `Tool` que em vez de rodar
localmente manda um `ToolCallRequest` pelo canal de escrita da conexão (o mesmo `mpsc` da 7.3) e
espera a resposta via um `oneshot` guardado num mapa `pending` (`call_id → oneshot::Sender`)
compartilhado por conexão (`RemoteToolChannel`) — várias tools anunciadas na mesma conexão dividem
um `next_call_id`/`pending` só, sem colisão. Timeout de 30s (`DEFAULT_TIMEOUT`) — uma leitura de
arquivo local não deveria demorar disso, diferente do `Chat` (7.3), que não tem timeout nenhum
porque uma resposta de modelo lenta é esperada. `server.rs`: depois do `Hello` validado, se `tools`
não for vazio, clona o `Orchestrator` compartilhado (barato, já é `Arc`-backed) e registra um
`RemoteTool` por spec anunciada — só essa conexão ganha essas tools; uma conexão sem nada anunciado
continua usando o `Orchestrator` do servidor inteiro, sem custo extra. Loop de leitura ganha dois
`match` novos (`ToolCallResult`/`ToolCallError` resolvem o `pending` pelo `call_id`) — a solução de
concorrência da 7.3 (task de escrita dedicada) não mudou nada.

**Bug real achado rodando o teste de ponta a ponta, não previsto no plano**: a primeira tentativa
nomeou as tools do celular `list_files`/`read_file` — os MESMOS nomes que `ReadFileTool`/
`WriteFileTool` (vault, `crates/warden-core/src/tool/file_tools.rs`) já usam, registradas por
`bootstrap()` em todo `Orchestrator`. A API do Gemini rejeitou a primeira chamada real com erro 400
"Duplicate function declaration found: read_file" — duas tools com nome idêntico na mesma lista de
`function_declarations` não é permitido. Renomeado pra `list_phone_files`/`read_phone_file`
(`mobile/lib/services/mobile_file_tool.dart`), sem tocar nos nomes do vault (`read_file`/
`write_file` já são convenção estabelecida em todos os canais). Lição registrada (P42): um nome de
tool de um cliente remoto pode colidir com uma tool já registrada localmente — na época, nada
detectava isso em `server.rs` (uma tool com nome duplicado simplesmente quebrava a chamada de API
do provider, silenciosamente do lado do Warden). **Resolvido na Sessão 91**, reaproveitando o
mesmo mecanismo que a Sessão 90 construiu pro caso análogo em MCP (P46): o loop de registro de
`RemoteTool` por conexão dedupa cada nome de `Hello.tools` contra o que já está no `Orchestrator`
compartilhado (`warden_core::tool::dedupe_tool_name`, promovida de `warden-bootstrap` pra pública),
namespaceando pelo `device_id` só quando há colisão de verdade (`{device_id}__{tool}`) via
`warden_core::tool::rename_tool`. Como `RemoteTool::call` manda o `ToolCallRequest` a partir do seu
próprio `spec` interno (nunca do que o wrapper reporta pro modelo), o cliente nunca precisa saber
do nome renomeado — continua recebendo o pedido pelo nome que sempre anunciou. Ver "Colisão de
nomes de tools entre MCP servers" e a entrada da Sessão 91 pro detalhamento técnico completo (o
mesmo texto vale pros dois lados, já que é a mesma função/wrapper).

**Pacote Flutter — achado de pesquisa antes de escrever código**: a escolha óbvia (`shared_storage`,
o wrapper mais conhecido do Storage Access Framework) está **descontinuada** no pub.dev, sem
sucessor listado na própria página. Rastreando o fork da comunidade (`mg_shared_storage`, também
descontinuado) até a recomendação dele mesmo, chegou-se em **`saf_util` + `saf_stream`**
(`github.com/flutter-cavalry`), par ativamente mantido (releases de poucos meses atrás) que faz a
mesma coisa dividida em dois pacotes — `saf_util` pra picker/listagem/permissão persistida,
`saf_stream` pra ler bytes de verdade. Confirmado lendo o código-fonte instalado em
`~/.pub-cache` (não só a doc do pub.dev) antes de escrever `mobile_file_tool.dart`.

**`mobile/lib/services/mobile_file_tool.dart`** (novo) — `pickRootFolder()`
(`SafUtil().pickDirectory(persistablePermission: true)`, persiste a URI via `shared_preferences`,
mesmo padrão de `connection_settings.dart`), `listFiles`/`readFile` (handlers reais das tools).
Design de "path opaco": cada entrada que `list_phone_files` devolve já carrega a URI SAF real do
documento como `path` — o modelo nunca constrói um path, só ecoa de volta um que já viu, mesmo
espírito de URI de recurso do MCP. Arquivo não-UTF-8 falha com erro claro em vez de estourar bytes
crus no chat.

**`mobile/lib/services/server_connection.dart`** — `connect`/`connectOverChannel` ganham
`toolSpecs`/`toolHandlers` opcionais (`Map<String, ToolHandler>`), inclusos no `Hello` só quando
não-vazios (opt-in, mesmo espírito do `enable_shell` do desktop). Novo case
`ToolCallRequestMessage` em `_onMessage` despacha pro handler registrado (`unawaited`, uma chamada
lenta não trava heartbeat/chat desta conexão) e manda `ToolCallResult`/`ToolCallError` de volta.

**UI** — botão de pasta na `AppBar` do `ChatScreen` abre um diálogo leve (não uma tela nova):
mostra a URI configurada, "Choose folder"/"Clear". Como o protocolo não tem "atualizar Hello
depois de conectado", trocar a pasta só tem efeito na próxima conexão — avisado no próprio diálogo.

**Testes**: `crates/warden-server/tests/tools.rs` — `MockProvider` estendido
(`calling_tool_then_replying`) pra devolver uma tool call na primeira chamada e derivar a resposta
final do conteúdo REAL da mensagem `Role::Tool` que voltou (não uma string enlatada — prova que o
resultado do cliente atravessou de verdade), round-trip completo contra um `Server` real + client
de teste cru. `remote_tool.rs` ganhou testes unitários próprios (sucesso, erro do cliente, timeout,
conexão caída) sem precisar de rede nenhuma. Lado Flutter: `flutter analyze`/`flutter test` cobrindo
o novo `Hello.tools`/`ToolCallRequest`/`ToolCallResult`/`ToolCallError` contra um
`StreamChannelController` fake. `cargo build/test/clippy --workspace` e `flutter analyze`/`flutter
test` limpos nos dois lados.

**Verificado de ponta a ponta contra um `warden-server` real com Gemini de verdade**: dois arquivos
de texto reais (`recipe.txt`/`notes.txt`) empurrados pro emulador via `adb push` numa pasta
`Download/warden-test`; picker real do Android (SAF) usado pra escolher essa pasta, diálogo de
permissão real aceito; pergunta real ("liste os arquivos, leia recipe.txt, me diga o ingrediente
secreto") respondida corretamente citando os dois arquivos reais e o conteúdo real de `recipe.txt`
("stardust", exatamente o que foi escrito no arquivo de teste) — prova a cadeia inteira (Gemini →
`Orchestrator` do servidor → `RemoteTool` → `ToolCallRequest` pela rede → SAF real no Android →
`ToolCallResult` de volta → resposta final) funcionando de verdade, não só nos testes automatizados.

## 8.1 + 8.2 — Extensão de navegador: setup + canal de chat (Sessão 68, continuação)

Usuário escolheu atacar a Fase 8 (extensão de navegador) fora da ordem do `ROADMAP.md`, que
colocava essa fase por último — escopo confirmado como só 8.1 (setup) + 8.2 (canal de chat), sem
as tools de DOM (8.3-8.6) nem publicação nas lojas (8.7-8.8).

**Boa notícia encontrada na pesquisa**: o protocolo servidor↔cliente (Fase 9.2,
`crates/warden-server-protocol`) já tinha um cliente de referência completo em Dart
(`mobile/lib/services/server_connection.dart`, Fase 7.2/7.3) fazendo exatamente o que a extensão
precisava — nada novo de protocolo, só porta pra TypeScript.

**Estrutura nova**: `extension/` na raiz do repo, irmão de `desktop/`/`mobile/`. Mesma stack de
`desktop/` (React 19, TypeScript ~5.8, Vite ^7) mais `@crxjs/vite-plugin` (`^2.7.1`, confirmado
compatível com Vite 7 antes de escolher — Vite puro não empacota Manifest V3 corretamente:
service worker/manifest precisam de tratamento especial que só um plugin dedicado dá).
Chrome-only nesta fatia — Firefox fica pra quando a publicação (8.8) existir, mesma postura "uma
plataforma primeiro" que a 7.1 do mobile teve com Android antes de iOS.

**Restrição de plataforma que definiu o desenho**: um popup de extensão MV3 é destruído toda vez
que fecha — a conexão WS *tem* que morar no **background service worker**, não no popup (decisão
estrutural, não de conveniência; também é o motivo de já valer a pena pra 8.3-8.6 futuras, já que
tool calls do DOM podem chegar com o popup fechado). Pesquisado antes de codar: Chrome 116+ reseta
o timer de ociosidade (~30s) do service worker a cada troca de mensagem pelo WebSocket — então um
heartbeat `Ping`/`Pong` a cada **20s** (mais apertado que os 30s do mobile, que ali só tratava de
NAT de operadora, não de manter o próprio processo do host vivo) evita que o Chrome descarte o SW
enquanto a conexão está ativa, sem precisar de `chrome.alarms`. Se o SW morrer mesmo assim (fechar
o Chrome, recarregar a extensão, máquina dormir), a conexão simplesmente some — igual ao gap já
aceito pela 7.2 do mobile ("sem reconexão automática"), só que aqui é o comportamento padrão da
plataforma, não uma escolha de escopo.

`extension/src/protocol/messages.ts` — tipos TS da união discriminada por `type`
(`ClientMessage`/`ServerMessage`), só o subconjunto que esta fatia usa (`hello`/`ping`/`chat`/
`goodbye` do lado cliente; `helloAck`/`authError`/`pong`/`chatResponse`/`chatError`/`goodbye` do
lado servidor — as variantes de tool call ficam de fora até 8.3 existir), com os nomes de campo
conferidos contra os testes que travam o formato JSON em `protocol.rs` (não adivinhados).
`extension/src/background/connection.ts` — porta 1:1 de `server_connection.dart`: handshake com
timeout de 10s, heartbeat, `sendChat`, `goodbye`, emissor de eventos de status mínimo (sem
dependência nova tipo `rxjs`). `extension/src/background/index.ts` — dono da única instância de
`ServerConnection` + histórico da conversa atual em memória (nunca persistido em
`chrome.storage` — se o SW morrer, a conexão morre junto, não é uma falha isolada a proteger),
`deviceId` gerado uma vez e persistido em `chrome.storage.local` (mesmo padrão
`getOrCreateDeviceId` de `mobile/lib/services/connection_settings.dart`), configurações de conexão
persistidas pra pré-preencher o formulário depois. **Achado durante a implementação**: a mensagem
do próprio usuário precisa ser ecoada de volta pro popup também (não só a resposta do modelo),
senão reabrir o popup no meio de uma conversa mostra só as respostas, nunca as perguntas —
`ChatEntry.role` ganhou `"user"` além de `"assistant"`/`"error"`, e o histórico em memória do
`index.ts` registra a mensagem de saída antes de mandar pro servidor.

`extension/src/popup/` — popup React (`ConnectionForm`/`ChatView`, decidido por `App.tsx` a
partir do status atual), comunica com o background via `chrome.runtime.sendMessage`/`onMessage`
(`background/popup_protocol.ts`, módulo de tipos só — deliberadamente separado de
`background/index.ts`, que registra um listener real de `chrome.runtime.onMessage` na carga do
módulo; importar isso dentro do bundle do popup registraria esse listener duas vezes, uma vez
sem sentido nenhum). CSS copiado (não importado) da paleta roxa de `desktop/src/App.css` — só o
essencial, já que `extension/`/`desktop/` são projetos JS independentes, mesma separação que
`desktop`/`mobile` já têm entre si.

**Verificação**: `npm install && npm run build` (tsc + crxjs/vite) limpo — manifest MV3 gerado
correto (`service_worker`/`action.default_popup`/`permissions: ["storage"]`), bundle do popup e
loader do service worker presentes em `dist/`. **Não verificado carregando a extensão de verdade
no Chrome nem contra um `warden-server` real rodando** — sem uma janela de Chrome disponível pra
interação manual neste ambiente e sem API key real configurada pro servidor; registrado como
lacuna igual às de sempre no projeto (P29/P30/P31 etc.), não fingido como testado. Ver
`PENDING.md`.

**Verificação de ponta a ponta real (Sessão 68, continuação 2)**: lacuna acima fechada com acesso
a automação de Chrome real disponível nesta sessão. Limitação encontrada: o Claude in Chrome não
consegue interagir com páginas internas do navegador (`chrome://extensions`/`brave://extensions`,
diálogos nativos de seleção de arquivo, nem páginas `chrome-extension://...`) — carregar a
extensão via "Load unpacked" e abrir/usar o popup de fato tiveram que ser feitos manualmente pelo
usuário; a automação ficou limitada a acompanhar o resultado pelo log do `warden-server`
rodando em paralelo (`cargo run -p warden-server --bin warden-server -- serve --auth-key ...
--provider gemini --model gemini-3.6-flash`, chave Gemini real do usuário). Confirmado dos dois
lados: log do servidor mostrou `Browser extension (<uuid>) connected from 127.0.0.1:...` no
handshake e o dispositivo de novo no `chat`; o popup mostrou a resposta real do modelo pra uma
mensagem de teste. Achado no caminho, sem relação com o código da extensão/servidor:
`gemini-2.5-flash` (modelo usado como exemplo nas sessões anteriores) foi descontinuado pela API
do Gemini (`404 NOT_FOUND`, "no longer available to new users") — trocado por `gemini-3.6-flash`
na hora. Fecha a parte de verificação do P67; o resto do escopo pendente da Fase 8 (8.3-8.6,
Firefox, reconexão automática, histórico persistido) segue igual. Usuário sinalizou, depois do
teste, que quer eventualmente uma UI de verdade pro popup — uma sidebar de chat configurável, no
espírito do Claude — registrado como ideia de polish futuro em `ROADMAP.md`, sem trabalho
iniciado.

## 8.3-8.7 — Tools de DOM na extensão de navegador (Sessão 68, continuação 3)

**Descoberta que definiu o escopo, confirmada antes de codar**: nenhum código novo era
necessário em `warden-server`/`warden-core`. O mecanismo genérico de "tool local por conexão"
(Fase 7.4, hoje só usado pelo mobile pra `list_phone_files`/`read_phone_file`) já cobre este caso
de ponta a ponta — qualquer cliente que anuncie `tools` não-vazio no `Hello` ganha,
automaticamente, um `RemoteTool` por spec registrado no `Orchestrator` daquela conexão
(`server.rs`), e o servidor já sabe rotear `ToolCallRequest`/casar `ToolCallResult`/
`ToolCallError` de volta com a chamada pendente do modelo. Todo o trabalho ficou em `extension/`.

**Permissão escolhida — `activeTab`, não `<all_urls>`**: decisão explícita do usuário, trade-off
de mínimo privilégio sobre poder irrestrito. `manifest.config.ts` ganhou `"scripting"` +
`"activeTab"` (sem `host_permissions`). Implicação documentada no código (`dom_executor.ts`):
`activeTab` só concede acesso à aba depois de um gesto do usuário (abrir o popup conta) e esse
acesso cai quando a aba navega — então depois de um `browser_navigate`, as tools seguintes na
mesma aba podem falhar até o usuário reabrir o popup. Vira um `ToolCallError` com mensagem
acionável, não uma falha silenciosa.

**4 tools novas**, prefixo `browser_` pra não colidir com `read_file`/`write_file`/`shell`/
`generate_document`/`delegate_task`/`delegate_to_agent`/`usage_stats` já registrados por
`bootstrap()` (lição da Fase 7.4: o mobile colidiu com `read_file` e o Gemini rejeitou a chamada
por nome duplicado — não há checagem de colisão em `server.rs`):
- `browser_read_page` — injeta uma função autocontida via `chrome.scripting.executeScript` que
  devolve título/URL/texto visível (truncado a ~4000 chars) e até 50 elementos interativos
  visíveis (`a`/`button`/`input`/`select`/`textarea`/`[role=button|link]`) cada um com um seletor
  CSS gerado (prioriza `#id`, senão caminho `tag:nth-of-type` até ficar único) — esse seletor é o
  contrato que `browser_click_element`/`browser_extract_text` esperam receber de volta do modelo.
- `browser_click_element` — `querySelector(selector)` + `scrollIntoView` + `.click()`; pega só o
  primeiro match, documentado na `description` da spec.
- `browser_navigate` — não usa `executeScript` (não roda no contexto da página): direto
  `chrome.tabs.update`, espera `tabs.onUpdated` bater `status: "complete"` com timeout de 10s.
- `browser_extract_text` — com `selector`, devolve `innerText` do elemento; sem `selector`,
  devolve a seleção de texto atual da página (`window.getSelection()`) se houver, senão o texto
  visível inteiro (mesmo teto de truncamento do `read_page`).

**Restrição do Chrome que moldou o código**: a função passada a `chrome.scripting.executeScript`
roda serializada, isolada, sem closures sobre módulos externos — cada tool define sua função
injetada como um closure autocontido (sem `import`), só trocando dado por `args`/`return`
(precisa ser serializável em JSON). `dom_executor.ts` centraliza a resolução da aba ativa e essa
chamada, pra não duplicar tratamento de erro nas 4 tools.

**`connection.ts`/`messages.ts`**: porte de `_handleToolCallRequest`/`ToolHandler` de
`server_connection.dart` — `hello.tools` deixou de ser `[]` fixo, `ClientMessage` ganhou
`toolCallResult`/`toolCallError`, `ServerMessage` ganhou `toolCallRequest`. Despacho em
`onMessage()` é fire-and-forget (sem `await` no `case`, mesmo padrão do `unawaited` do Dart) —
uma tool lenta (ex. ler uma página grande) não trava heartbeat/chat da mesma conexão.

**Bug real encontrado e corrigido no caminho, sem relação com a extensão em si**: a primeira
verificação de ponta a ponta (`browser_read_page` de verdade, contra `gemini-3.6-flash`) voltou
um `400 INVALID_ARGUMENT`: *"Role 'function' is not supported. Please use a valid role: SYSTEM,
..., MODEL, USER."* `crates/warden-core/src/model/gemini.rs`'s `to_content` mandava
`Role::Tool` como `role: "function"` — role que a API do Gemini nunca aceitou de fato (pesquisa
confirmou: `contents` não tem role dedicado pra resultado de function call; uma `functionResponse`
part viaja dentro de um turno `role: "user"`). Esse caminho nunca tinha teste de regressão
cobrindo `to_content(Role::Tool)` — corrigido pra `role: "user"` e um teste novo
(`a_tool_result_is_sent_as_a_user_turn`) trava o formato certo. **Bug pré-existente, não
introduzido por esta fatia** — só nunca tinha sido exercitado contra Gemini antes (mobile/outras
sessões usaram outro provider ou nunca completaram um round-trip de tool call real contra
Gemini), mas afeta qualquer tool call via Gemini, não só as tools de DOM.

**Verificação de ponta a ponta real**: mesmo par Brave real + `warden-server` real da Sessão 68
(automação de página segue sem alcançar `chrome://extensions`/popup da extensão — passos manuais
do usuário, como antes). Pedido no chat forçando `browser_read_page`, respondido corretamente
depois da correção do role do Gemini acima. Não testado nesta rodada: `browser_click_element` /
`browser_navigate` / `browser_extract_text` individualmente contra uma página real (só
`browser_read_page` foi exercitado pelo usuário) — ficam cobertos pela mesma revisão de código e
pelo mesmo mecanismo genérico, mas sem confirmação manual própria; e o caso de erro esperado
depois de um `browser_navigate` (permissão `activeTab` caindo) não foi provocado de propósito.
Anotado como lacuna menor em `PENDING.md`.

**Sem UI nova no popup nesta fatia** — o modelo chama as tools direto durante o chat, sem
indicador visual de "ação no DOM em andamento". Fecha 8.7 como efeito colateral direto (era só o
roteamento client-side que os itens acima já implementam).

---

## Skills no vault, ativadas sob demanda (P16, Sessão 73)

**Decisão**: uma skill é um arquivo `skills/<nome>.md` no vault (frontmatter `name`/`description` +
corpo em markdown). O modelo só vê o catálogo (nome + descrição) a cada turno e carrega o corpo
chamando a tool `use_skill(nome)` quando a skill se aplica.

**Por que no vault e não no `config.toml`**: o vault já sincroniza (`warden-sync`/git usam
`Vault::list_all_files`), é editável no Obsidian, e o `config.toml` viaja inteiro no bundle de sync
junto com as API keys — além de o `deny_unknown_fields` de `FileConfig` quebrar clientes antigos que
recebessem um `[[skills]]` desconhecido.

**Por que sob demanda, e global por padrão**: skill não usada custa zero token; funciona em
qualquer canal (só desktop e CLI têm o conceito de agente, ver P45); e é o mesmo modelo mental das
Skills do Claude. (A restrição opcional a agentes veio depois, na Sessão 75 — ver abaixo.) O catálogo é lido do vault a cada turno, então uma skill criada no meio da
conversa aparece no turno seguinte sem reconstruir o `Orchestrator`.

**Detalhes que importam**:
- O nome da skill é o nome do arquivo, então é validado como slug (`[a-z0-9-]{1,64}`) em
  `warden_core::skill::validate_name` — `Vault::write` não protege contra `../`, esta validação é o
  que segura o caminho.
- `skills/` fica fora de `Vault::list_files`/`search`/`search_semantic` (o corpo é instrução, não
  memória a ser citada como hit), mas dentro de `list_all_files` — por isso o sync as leva sem
  nenhuma mudança em `warden-sync`.
- O catálogo só é injetado se `use_skill` estiver registrada no `Orchestrator`, pra não anunciar uma
  tool inutilizável.
- `manage_skill` aceita só `create`/`update` (create recusa nome já existente, update recusa nome
  inexistente). **Sem `delete` pra IA, de propósito**: um erro do modelo só pode sobrescrever, nunca
  apagar em silêncio uma skill escrita pelo usuário; apagar é só na UI.
- O gerador por prompt (`warden_bootstrap::skill_gen`) devolve um rascunho **sem salvar** — o
  usuário revisa no formulário. O parse é tolerante (cerca ```json, preâmbulo, nome com espaços é
  slugificado) e passa pela mesma validação de uma skill escrita à mão.
- **Mobile (P72 b)**: a tela de skills do celular chama `warden_core::skill::SkillStore` pela ponte
  Rust (`api/skills.rs`) em vez de reimplementar o parse do frontmatter e as regras de validação em
  Dart — uma fonte só de verdade, e um skill rejeitado volta como a mesma mensagem de erro do desktop/CLI.
  Sem o gerador por prompt: o celular não tem modelo próprio.
- Correção de premissa registrada: agentes não têm criação por IA nem por prompt (só formulário no
  Settings e `/agents` no CLI); esses dois caminhos foram construídos do zero pra skills.

**Skill vinculada a agente (P72 c, Sessão 75)**: o frontmatter ganha `agents: a, b` (uma linha, ids
separados por vírgula; ausente/vazio = global, como antes — arquivos existentes não mudam). O
catálogo e o `use_skill` filtram pelo agente ativo do turno: `Orchestrator::with_agent(id)` (mesmo
estilo de `with_model`/`with_tool`) guarda o id e troca a tool `use_skill` por uma escopada
(`UseSkillTool::for_agent`). **Sem agente ativo (Telegram, WhatsApp, servidor/extensão, mobile) só as
skills globais aparecem.** Uma skill de outro agente responde "no skill named" — indistinguível de
inexistente. Quem ativa: `send_message` do desktop, o loop do CLI e o `delegate_to_agent`
(`build_delegate_to_agent_tool` chama `with_agent` em cada alvo, que vê as skills dele e não as do
chefe). Limitação: id de agente com vírgula não cabe no formato de uma linha (`Skill::validate` recusa).
Quem não edita `agents` (ponte do mobile, extensão, `manage_skill` sem o parâmetro) **preserva** o valor
gravado ao sobrescrever — senão salvar pelo celular tornaria a skill global de novo, em silêncio.

**Arquivos anexos (P72 d, Sessão 76)**: uma skill pode carregar arquivos de texto (scripts, modelos,
notas) na pasta companheira `skills/<nome>.files/<arquivo>`, sem mudar o frontmatter — skill sem anexo
não muda. A pasta não vira skill (`list` só aceita `*.md` e nome de skill não tem ponto), o sync já a leva
(`list_all_files` pega qualquer extensão sob `skills/`) e a busca a ignora (`skills/` fica fora do
`list_files`). Nome do anexo: `[A-Za-z0-9._-]`, sem ponto inicial, ≤ 64 chars (tira `/`, `..` e dotfile por
construção); só texto (`Vault::read/write` são `String`), ≤ 64 KB por arquivo, ≤ 20 por skill, sem
subpastas. `SkillStore` ganhou `list_files/read_file/read_file_for/save_file/delete_file`; apagar a skill
apaga a pasta. **O modelo**: `use_skill` devolve também `files: [{name, path}]` (só quando há anexos; `path`
relativo ao vault, pra rodar no `shell`), a tool nova `read_skill_file(skill, file)` lê um deles com o
mesmo filtro por agente (`with_agent` troca as duas), e `manage_skill` aceita `files: [{name, content}]`
(upsert; sem o campo os anexos ficam intactos; sem delete pela IA, mesma razão do corpo). **Executar um
script não tem caminho próprio**: só o `shell` existente, opt-in e sem sandbox — a skill não amplia o que
o modelo já podia fazer. Desktop: comandos `list_skill_files/read_skill_attachment/save_skill_attachment/
delete_skill_attachment`, separados do `SkillPayload` (anexo só existe pra skill salva; gravam na hora, sem
esperar o "Save skill"). CLI: `/skills file|attach|detach`. Extensão/mobile/servidor não ganharam UI nem
DTO: `save` só reescreve o `.md`, então editar por lá **preserva** os anexos.

**Skills pela extensão (P72)**: o vault mora no servidor, então o protocolo ganhou
`ClientMessage::{ListSkills, SaveSkill, DeleteSkill}` (com `request_id` de correlação, como o
`call_id` do `CallDeviceTool`) e `ServerMessage::{SkillList, SkillOk, SkillError}`, mais o `SkillDto`.
`warden-server/src/skills.rs` aplica as mesmas regras do desktop (criar recusa nome usado, editar
sobrescreve) sobre o `SkillStore` do vault hospedado, respondendo inline (E/S curta, sem `spawn`).
Não passam pelo gate de pareamento do `CallDeviceTool`: quem tem a `auth_key` já pode mandar o modelo
escrever no vault via `write_file`, então gatear só as skills não protegeria nada.


## SSH em servidores externos (P47, Sessão 77)

A tool `ssh_exec` roda um comando num servidor cadastrado em `config.toml` (`[[ssh_hosts]]`, editável
no desktop em Settings → "SSH servers" e no CLI por `/ssh`). Decisões confirmadas com o usuário antes de
codar (plano aprovado):

- **Transporte = o binário `ssh` do sistema**, não uma lib SSH em Rust. Mesmo padrão do `warden-sync`,
  que chama o `git`: herda `known_hosts`, ssh-agent e `~/.ssh/config` sem dependência nova. Custo: exige
  OpenSSH instalado (Linux/Mac sempre, Windows 10+; só o Linux foi testado).
- **Controle = liberar por host e por agente, comando livre.** Cada host tem `enabled` (default `false`)
  e `agents` (vazio = todos os agentes **e** canais sem agente — Telegram, WhatsApp, mobile, MCP server —
  a mesma confiança do `shell`). Sem prompt de aprovação por comando e sem allowlist de comandos: o
  orquestrador não tem confirmação por chamada em nenhum canal, e filtrar prefixo de string é fácil de
  burlar com `;`/`&&`.
- **A chave privada nunca entra no `config.toml`**: só o caminho (`identity_file`). Sem passphrase
  guardada; chave protegida usa o ssh-agent.
- **O modelo só escolhe um `host_id`**, nunca hostname nem usuário. `spec()` é dinâmica e lista só os
  hosts que o agente atual pode usar (o `host_id` vira um `enum` no schema).
- **Escopo por agente** passa pelo mesmo ponto do `use_skill`, `Orchestrator::with_agent`, mas via dois
  métodos novos com default no trait `Tool`: `scoped_to_agent` (devolve uma cópia da tool para o agente)
  e `is_available` (o orquestrador não anuncia a tool ao modelo se ela devolver `false`, ex. agente sem
  host visível). A cópia mantém a lista completa de hosts e só troca o agente, então re-escopar
  (`with_agent` em cima de `with_agent`) nunca perde hosts.
- **Registro**: `bootstrap()` só registra `ssh_exec` se houver ao menos um host `enabled` e válido
  (`build_ssh_tool`); sem flag global, cada host já tem a sua chave.

**Endurecimento (o que a chamada `ssh` não faz por padrão)**: `BatchMode=yes` (nunca fica esperando
senha/passphrase), `StrictHostKeyChecking=yes` (host key desconhecida é erro com dica, nunca é aceita
sozinha — o usuário confia na chave rodando `ssh` uma vez num terminal), `ConnectTimeout=10`, `--` antes
do host. `host`/`user` só aceitam `[A-Za-z0-9._:%-]`/`[A-Za-z0-9._-]` e não começam com `-` (barra
`-oProxyCommand=…` e URI `ssh://…`, que o `ssh` aceita na posição do host e que sobrescreve `-l`/`-p`),
validados ao salvar **e** a cada chamada. **Verificado com o `ssh` real (`ssh -G`, OpenSSH 10.5)**: sem
`--`, uma opção depois do hostname é aplicada (`-oProxyCommand=…` rodaria código **local**); com `--`,
não é. Então o `--` já protege nesta versão, e `run_on_host` recusa também um `command` que comece com `-`
como defesa em profundidade, porque só esta versão foi testada (outras versões do OpenSSH, o port do
Windows ou um wrapper no PATH podem interpretar diferente).

**Achado de fail-open no desktop (corrigido antes de fechar)**: apagar um agente podava o id das listas
dos hosts; um host restrito só àquele agente ficava com lista vazia = "todos os agentes". Agora a poda
que esvazia a lista **desliga** o host (`enabled = false`). No CLI, `/agents remove` não mexe nos hosts:
o host continua citando um agente que não existe, e ninguém o alcança (falha fechada).

O CLI registra as tools uma vez na inicialização (diferente de agentes/modelos, relidos a cada turno), então
mudanças por `/ssh` só valem na próxima vez que o Warden inicia; o desktop reconstrói o orquestrador ao
salvar as Settings.

### Transferência de arquivos, auditoria e aprovação (P47, Sessão 79)

`ssh_exec`, `ssh_upload` e `ssh_download` são um só tipo (`SshTool`) sobre um `SshContext`: mesma lista de hosts,
mesmo escopo por agente, mesma aprovação, mesmo log. Decisões confirmadas com o usuário antes de codar:

- **Transferência sobre o `ssh`, sem `scp`/`sftp`**: o arquivo é o stdin (upload) ou o stdout (download) do
  próprio `ssh`, e o remoto roda `sh -c 'set -C; cat > <path>'` / `sh -c 'cat -- <path>'`. Reusa `ssh_args`
  (mesmo endurecimento) e não traz outro parser de opções. O caminho remoto passa por `shell_quote` (aspas simples
  com escape) e o comando vai dentro de `sh -c` para não depender do shell de login (fish/csh não leem `set -C`).
  Exige `sh` e `cat` no remoto.
- **Sem sobrescrever por engano**: upload usa `set -C` (noclobber) e o download recusa destino existente, ambos
  liberados só com `overwrite=true`. O download grava em `<destino>.part` e renomeia no sucesso, então conexão
  caída, timeout ou arquivo acima do teto (100 MB) não deixam um arquivo pela metade.
- **Caminho local sem sandbox** (decisão explícita): relativo à raiz do vault ou absoluto, a mesma confiança do
  `shell` e dos file tools. O freio é a aprovação por host, e cada transferência entra no log.
- **Auditoria**: `~/.config/warden/ssh_audit.jsonl`, uma linha por chamada — inclusive as recusadas
  (`approval` = `not_required|approved|denied|unavailable`). Grava comando/caminhos e o resultado, **nunca
  stdout/stderr**. Arquivo `0600`. Falha ao gravar vira aviso, nunca derruba a chamada. Sem rotação.
- **Aprovação por comando**: `require_approval` por host. `Approver` (trait no core) + `Tool::with_approver` +
  `Orchestrator::with_approver`, o mesmo molde do escopo por agente. Só o **CLI interativo** e o **desktop**
  passam um approver; **todo outro caminho recusa** (falha fechada): CLI não interativo, Telegram, WhatsApp,
  mobile, MCP server e sub-agentes (`delegate_task` tem o próprio orquestrador, sem approver). Sem resposta em
  120 s = recusa. Entrada inválida é recusada antes de perguntar.
- **Por que só esses dois canais**: o orquestrador roda o loop de tools inteiro dentro de um `handle_turn`, então o
  approver precisa de um caminho de volta até quem é dono da tela. No CLI o turno roda numa task e o laço de
  `run_turn` (dono do terminal) recebe o pedido por um `mpsc` + `oneshot`; no desktop é um evento Tauri e um
  comando (`resolve_approval`, ex-`resolve_ssh_approval`). Telegram/WhatsApp precisariam de botão inline ou resposta em mensagem, e não
  deu para testar contra os serviços reais.

## Agentes criam agentes: `manage_agents` (P46, Sessão 80)

Tool no `warden-bootstrap` (`manage_agents.rs`), não no core: precisa de `AgentConfig`, `load_config_from_path`
e `save_config`, como o `UsageStatsTool`. Decisões confirmadas com o usuário antes de codar:

- **Opt-in por agente e aprovação sempre**: só um agente com `can_manage_agents` recebe a tool, anexada por turno
  pelo desktop (`send_message`) e pelo CLI (`resolve_turn_context`), exatamente como `delegate_to_agent` — os únicos
  dois canais que resolvem `agent_id`. Toda criação/edição passa pelo `Approver` (o mesmo do P47); sem approver
  recusa. Como Telegram/WhatsApp/mobile/MCP server nunca anexam a tool, não há caminho sem tela para ela.
- **Poder não se auto-concede** (aplicado no código): `create` sempre grava `can_manage_agents=false` e
  `can_delegate_to_agents=false` e ignora qualquer argumento extra; `update` só mexe em persona/provider e **recusa**
  editar um agente que tenha alguma das duas flags, inclusive o próprio chamador. Só as checkboxes de Settings / o
  wizard do CLI ligam essas flags. Sem `delete` na v1 (mesmo raciocínio do `manage_skill`); a Sessão 83 acrescentou
  o `delete`, ver a seção "`delete` no `manage_agents`" abaixo.
- **O revisor vê tudo**: a persona (limite de 4000 caracteres, justamente para caber) aparece inteira no pedido; no
  `update` aparece a persona antiga e a nova. Entrada inválida (id vazio/com espaço/controle, duplicado, provider
  inexistente, persona vazia ou grande) é recusada **antes** de perguntar.
- **Sem corrida com o prompt aberto**: `plan()` é pura e roda duas vezes — antes de perguntar (valida e monta o
  texto) e **depois** do "sim", sobre o config relido do disco. O que outro processo salvou enquanto o prompt estava
  aberto não se perde (só `agents` é substituído, o resto do `FileConfig` é o que está no disco), e um nome tomado
  nesse intervalo é pego. Continua uma leitura-e-regravação do arquivo inteiro: um `save_settings` do desktop no
  mesmo instante ainda pode vencer.
- **Vale a partir do próximo turno**: agentes são relidos no início de cada turno, e `delegate_to_agent` é montado
  nesse momento, então o agente criado não é alvo de delegação no mesmo turno. O chat do desktop relê os settings
  depois de cada resposta (mantendo o mesmo objeto quando nada mudou, porque o efeito que restaura o seletor
  depende dele).
- **Approver generalizado**: `ApprovalRequest { target, action, detail }` (antes `host_id`). No desktop,
  `approval.rs` (`ApprovalBroker`, `TauriApprover`, `resolve_approval`, eventos `approval-request`/
  `approval-cancelled`) e `ApprovalModal.tsx`, com verbos por `action` (`exec`, `upload`, `download`,
  `create_agent`, `update_agent`). No CLI o card mostra uma linha por linha do `detail`.
- **Achado**: o renderizador de markdown do card do CLI consome os `_` (`manage_agents` aparece como
  `manageagents`); é anterior a esta mudança e só afeta a exibição.

## Isolamento de tools por agente: `allowed_tools` (P46, Sessão 81)

Lista permitida de tools por agente nomeado, aplicada no código (não pedida ao modelo). Decisão confirmada com o
usuário: um agente **criado por outro agente** sem lista recebe só o conjunto de leitura.

- **Modelo**: `AgentConfig.allowed_tools: Option<Vec<String>>` (`#[serde(default)]`). `None` = todas as tools
  (configs antigas e o comportamento anterior); `Some(lista)` = só essas, e `Some([])` é um agente que só conversa.
  `delegate_to_agent` e `manage_agents` **não** entram na lista: seguem só as flags `can_*` (a lista rejeita os dois
  nomes), e são anexadas depois do filtro.
- **Onde é aplicado**: `Orchestrator::with_allowed_tools(Option<&[String]>)` remove do orquestrador as tools fora da
  lista — some do spec anunciado e uma chamada forçada vira "unknown tool". Desktop (`send_message`) e CLI
  (`resolve_turn_context` devolve agora um `TurnContext` com `allowed_tools`) aplicam logo depois de `with_agent`.
- **Bypass fechado — `delegate_task`**: o `DelegateTool` guarda um sub-orquestrador com todas as tools, então um
  agente restrito que ainda pudesse delegar escaparia da lista. Novo método `Tool::restricted_to(&[String])` (padrão
  `None`), implementado só pelo `DelegateTool`, estreita o orquestrador interno com a mesma lista (recursivo, porque
  o interno tem o seu próprio `DelegateTool`).
- **Ordem importa — alvos de `delegate_to_agent`**: `build_delegate_to_agent_tool` aplica **a lista de cada alvo**
  (`with_agent(id).with_allowed_tools(alvo.allowed_tools)`), e por isso recebe o orquestrador **antes** de ele ser
  estreitado para o agente ativo; senão todo alvo herdaria os limites do chefe (e nunca poderia ter mais que ele).
- **`manage_agents` e a regra "ninguém dá o que não tem"**: `create` sem `allowed_tools` grava `SAFE_AGENT_TOOLS`
  (`read_file`, `use_skill`, `read_skill_file`, `usage_stats`, `generate_document` — sem `write_file`, `shell`,
  `ssh_*`, `manage_skill`, `delegate_task` nem MCP), cortado pela lista do próprio chamador. Uma lista pedida só pode
  citar tools que existem (`with_known_tools`, nomes do orquestrador em execução) e só as que o chamador também tem
  (`with_caller_limit`); tudo é validado antes de perguntar e o card mostra a lista (no `update`, antiga → nova).
  `list` devolve `allowed_tools` de cada agente, `grantable_tool_names` e `default_tools_for_new_agents`.
- **Superfícies**: Settings do desktop ganhou "Restrict tools" + checkboxes (comando novo `list_tool_names`; uma tool
  da lista que sumiu, como a de um MCP fora do ar, aparece como "(not available now)" até ser desmarcada); o wizard do
  CLI ganhou o campo "tools permitidas" (vírgula, em branco = todas) e `/agents` mostra `[tools: N]`.
- **Limitações aceitas**: a lista é por **nome** — dois MCP servers com uma tool de mesmo nome não se confundem mais
  (ver "Colisão de nomes de tools entre MCP servers" abaixo, Sessão 90), mas ainda exige que quem escreve
  `allowed_tools` saiba o nome renomeado quando existe colisão; Telegram/WhatsApp/mobile/MCP server não têm agente
  nomeado, então seguem com todas as tools; as tools `ssh_*` já tinham o próprio escopo por host/agente e continuam
  com ele além da lista.

## Colisão de nomes de tools — MCP servers (P46) e clientes remotos (P42) (Sessões 90-91)

Até a Sessão 90, `register_mcp_tools` (`warden-bootstrap`) registrava cada tool de um `[[mcp_servers]]` com o nome
cru que o servidor reporta, sem checar nada contra o que já estava registrado (tools nativas, Tavily, ou um
`[[mcp_servers]]` anterior). O despacho do `Orchestrator` resolve por `tools.iter().find(|t| t.spec().name ==
tool_call.name)` — o **primeiro registrado sempre ganha**; um nome repetido deixava a segunda tool inalcançável pra
sempre, sem aviso, e a lista de `ToolSpec` mandada ao provider ficava com dois nomes iguais (a maioria das APIs de
function-calling rejeita ou se confunde com isso). A Sessão 91 achou e fechou o mesmo bug do outro lado: um cliente
remoto (celular, extensão de navegador) que anuncia em `Hello.tools` um nome que já existe no `Orchestrator`
compartilhado (P42, achado original na Sessão 52) tinha exatamente o mesmo problema — hoje os dois casos usam o
mesmo mecanismo, descrito uma vez aqui.

- **Só renomeia quando colide de verdade.** Nunca por via das dúvidas — uma tool sem conflito mantém exatamente o
  nome de hoje, pra não invalidar `allowed_tools`/skills/hábitos já escritos. `dedupe_tool_name(existing, namespace,
  tool)` (pura, **`warden_core::tool`** — pública desde a Sessão 91, antes vivia privada em
  `warden-bootstrap`, movida pra ser a mesma fonte de verdade dos dois call sites) devolve o nome intacto se nada
  mais o usa, ou `"{namespace}__{tool}"` na colisão — `__`, não `.`/`-`, porque OpenAI/Gemini/Anthropic restringem
  nome de função a `[a-zA-Z0-9_-]`. `namespace` é o nome do `[[mcp_servers]]` (P46) ou o `device_id` do cliente
  conectado (P42, `crates/warden-server/src/server.rs`). O primeiro a registrar um nome sempre fica com a versão
  crua; quem colide depois (outro `[[mcp_servers]]`, um cliente remoto, ou qualquer um dos dois anunciando um nome
  que já é de uma tool nativa) é que ganha o prefixo.
- **`NamespacedTool`/`tool::rename_tool(tool, name)`** (`warden-core/src/tool/mod.rs`): wrapper privado de `Tool`
  que só sobrescreve `spec().name`, delegando `call`/`is_available` direto. As 5 outras "copie este tool, mas..."
  do trait (`scoped_to_agent`/`restricted_to`/`with_budget`/`with_jobs`/`with_approver`) delegam pro tool interno
  **e re-envolvem** o resultado com o mesmo nome — sem isso o nome renomeado se perderia assim que
  `with_allowed_tools`/`with_budget`/etc. produzisse uma cópia nova do tool.
- **`register_mcp_tools` virou genérica sobre `ToolProvider`** (`async fn register_mcp_tools<P: ToolProvider>(...)`)
  em vez de amarrada a `McpToolProvider` — só usava `tools()`, o método do trait. Os dois call sites reais (Tavily,
  o loop de `config.mcp_servers`) continuam passando `McpToolProvider`, inferido; o que essa generalização abre é
  testar a função de verdade com um `ToolProvider` fake, sem precisar de um processo MCP real. Uma renomeação
  imprime uma nota no stderr (nome antigo → novo, servidor de origem), mesmo estilo de "MCP server unavailable" já
  existente — é como quem escreve `allowed_tools` descobre o nome a usar.
- **Fora de escopo, documentado**: nenhuma UI/comando lista as renomeações feitas (o aviso no stderr no startup é o
  mecanismo de descoberta, consistente com todo o resto do graceful-degradation de MCP); colisão **dentro** do mesmo
  server (duas tools do próprio `tools/list` com nome igual) é tratada sem pânico pelo mesmo mecanismo, mas é bug do
  server, não um caso pensado especialmente.
- **Lado cliente remoto (P42, Sessão 91)**: `crates/warden-server/src/server.rs`'s loop que registra um `RemoteTool`
  por tool anunciada em `Hello.tools` (Fase 7.4) dedupa contra `per_connection.tools()` antes de registrar, com o
  `device_id` do cliente como `namespace`. Funciona sem mudar nada no protocolo nem nos clientes porque
  `RemoteTool::call` (`crates/warden-server/src/remote_tool.rs`) manda o `ToolCallRequest` a partir do seu **próprio**
  `spec` interno, fixado uma vez em `RemoteTool::new` — o wrapper `NamespacedTool` só troca o que `spec()`
  *reporta* pro modelo/despacho, nunca o que a chamada real manda pro cliente. Também não existe colisão **entre**
  dois clientes diferentes: cada conexão com `Hello.tools` ganha seu próprio clone do `Orchestrator`
  (`per_connection`), então a tool de um cliente nunca aparece no `Orchestrator` de outro — a única colisão possível
  é entre a tool de **um** cliente e o que já está na base compartilhada (vault/shell/SSH/MCP servers), exatamente o
  caso real que originou o P42 (celular vs. `ReadFileTool`/`WriteFileTool`). Testado com um servidor e uma conexão
  reais sobre WebSocket (`crates/warden-server/tests/tools.rs`, `spin_up_server_with_base_tool` novo em
  `tests/support/mod.rs`) — confirma tanto o rename quanto que o cliente nunca vê o nome novo.

## Teto de custo dos sub-agentes: `TurnBudget` (P46/P60/P18, Sessão 82)

Sem fila de jobs nem controle de custo, o pior caso de uma delegação recursiva era `MAX_TOOL_ITERATIONS ^ profundidade`
chamadas de modelo (P60), e o uso de tokens dos sub-agentes era descartado (P18). Agora cada turno tem um orçamento.

- **Unidade: chamadas de modelo, não tokens.** O número de chamadas é determinístico e se checa **antes** de gastar;
  token só se sabe depois e nem todo provider reporta. Os tokens são só **contabilizados**, não limitados.
- **`TurnBudget`** (`warden-core/src/budget.rs`): contador atômico + uso somado, compartilhado (`Arc`) pela **árvore
  inteira** do turno — `delegate_task`, `delegate_to_agent` e o que eles delegam. Uma cadeia raiz → nível 1 → folha gasta
  do mesmo orçamento (teste `a_chain_of_sub_agents_shares_one_budget`).
- **Só sub-agentes são cobrados.** O orquestrador em que o turno começou nunca é cobrado (já é limitado por
  `MAX_TOOL_ITERATIONS`). Assim, ao esgotar, o sub-agente falha e o erro chega ao pai como resultado de tool
  (`error: … limit of N model calls …`); o pai continua e responde com o que tem, em vez de o turno inteiro falhar.
- **Um orçamento novo por turno, em todos os canais**: `handle_turn_streaming` cria o `TurnBudget` quando o
  orquestrador tem `delegation_limit` e ainda não carrega um orçamento (o sub-agente já carrega o do pai). Por isso
  vale no desktop, CLI, Telegram, WhatsApp, mobile e MCP server sem mexer na montagem de cada um. O orçamento é
  distribuído com `Tool::with_budget` (mesmo molde de `with_approver`/`restricted_to`), implementado por `DelegateTool`
  e `DelegateToAgentTool` via `Orchestrator::charged_to`. Como os tools são mapeados no início do turno, o que foi
  anexado por turno (`delegate_to_agent`, `manage_agents`) também entra.
- **P18 fechado sem mudar `Tool::call`**: cada chamada de sub-agente registra o `usage` no orçamento e o raiz soma o
  total ao `MessageOutcome.usage` no fim. Isso alimenta sozinho o `/usage` do CLI e o que o desktop grava por mensagem
  (a tela Usage passa a contar o gasto dos sub-agentes, atribuído ao agente da conversa).
- **Configuração**: `max_delegated_calls` em `config.toml` / `WARDEN_MAX_DELEGATED_CALLS` (env vence arquivo, valor
  malformado cai no arquivo, mesmo molde de `delegate_max_depth`); padrão `DEFAULT_MAX_DELEGATED_CALLS = 30`; **`0`
  desliga o teto** (escolha explícita, como o `delegate_max_depth` sem clamp). Sem UI; `save_settings` do desktop carrega o
  valor de `existing` para não apagá-lo a cada save.
- **Limitações aceitas**: o trabalho parcial de um sub-agente cortado no meio se perde (só o erro volta); o teto é por
  turno, não por período/usuário (isso continua sendo o P4); `Orchestrator::new` sem `with_delegation_limit` (testes,
  `warden-mcp-server` montado à mão) segue sem teto e sem somar o uso dos sub-agentes.

## `delete` no `manage_agents` (P46, Sessão 83)

A v1 do `manage_agents` não tinha `delete` ("um erro do modelo só pode sobrescrever, nunca perder algo em silêncio").
Agora tem, porque todo delete passa pela aprovação humana e o card mostra a persona **inteira** que será perdida.

- **Mesmas regras de poder**: recusa id inexistente e agente com `can_delegate_to_agents`/`can_manage_agents` (o próprio
  chamador tem a flag, então não se apaga) — só uma pessoa apaga um agente com poder. As recusas vêm **antes** de
  perguntar, e o delete é reaplicado sobre o config relido do disco depois do "sim", como create/update.
- **Apagar não é só tirar da lista — hosts SSH**: `ssh_hosts[].agents` guarda ids de agente. Referência pendurada faz o
  `save_settings` do desktop falhar ("names an unknown agent"), e podar deixando a lista vazia **alargaria** o acesso
  (vazio = todos os agentes e canais). `remove_agent_from`/`remove_agent_references` (`warden-bootstrap`) tiram o id
  dos hosts e, se sobra lista vazia, **desligam** o host — a mesma regra que o `deleteAgent` da tela de Settings já
  tinha. A tool e o `/agents remove` do CLI usam a mesma função (o CLI antes só fazia `retain` e deixava a referência
  pendurada, o que quebrava o próximo save do desktop: bug corrigido junto).
- **`plan()` devolve `Planned { agents, ssh_hosts, detail }`**: o delete é o primeiro `Change` que mexe fora de `agents`.
  O card descreve o efeito em cada host e avisa que o SSH só é relido na próxima inicialização.
- **Skills restritas ao agente ficam como estão** (`agents` no frontmatter): pendurado significa "visível a ninguém", o lado
  seguro; recriar um agente com o mesmo nome passa pela aprovação de novo.
- **Desktop**: só o verbo `delete_agent` no `ApprovalModal`. **Não desfaz**: o único registro da persona apagada é o
  card mostrado antes do "sim".

## Agente criado no meio do turno já é alvo de delegação (P46, Sessão 84)

Até a Sessão 83 um agente criado por `manage_agents` só virava alvo de `delegate_to_agent` no turno seguinte: a tool era
montada no início do turno com a lista fixa, e o `Orchestrator` calculava as specs das tools **uma vez**, antes do loop.

- **Duas causas, dois ajustes**: (1) `handle_turn_streaming` agora recalcula `tool_specs` a **cada iteração** do loop
  (só chamadas a `spec()`, baratas), então uma spec que muda no meio do turno chega ao modelo; (2) o
  `DelegateToAgentTool` ganhou um modo "vivo" (`DelegateToAgentTool::live`) que refaz a lista de alvos.
- **`AgentsRevision`** (`warden-core`, `Arc<AtomicU64>`) é o aviso de que a lista mudou: o `ManageAgentsTool` chama
  `bump()` **depois** de `save_config` bem-sucedido; a tool de delegação compara com a revisão que viu e, se mudou,
  chama o `AgentResolver` (`Fn() -> Option<Vec<NamedSubAgent>>`). O resolver **só roda quando a revisão mudou**, não a
  cada `spec()` (o `run_tool` chama `spec()` de todas as tools a cada chamada de tool). Sem contador compartilhado
  nada muda: o comportamento é o de antes.
- **Por que contador e não olhar o arquivo**: mtime tem granularidade e corrida; um contador explícito é exato e nunca
  relê o `config.toml` sem motivo. Recusa, erro de validação e `list` **não** movem a revisão (teste dedicado).
- **`build_live_delegate_to_agent_tool`** (`warden-bootstrap`): o resolver relê o `config.toml` e reaproveita a mesma
  `delegate_targets` que já montava os alvos (extraída de `build_delegate_to_agent_tool`, que continua existindo para
  quem não tem `manage_agents`). Os alvos recarregados clonam o **mesmo** `orchestrator` de base, então têm as mesmas
  tools/profundidade e a `allowed_tools` própria; e passam por `charged_to` quando o turno tem `TurnBudget`, então o
  teto de custo continua valendo pra eles (`with_budget` propaga o orçamento e mantém o modo vivo).
- **Resolver sem resultado mantém a lista anterior** (config ilegível ou lista vazia): um tropeço no meio do turno não
  tira a delegação. Efeito colateral aceito: se a lista ficasse vazia de verdade, o agente apagado continuaria
  endereçável até o fim do turno — na prática impossível, porque o próprio chefe (com poder) está sempre na lista e
  não pode ser apagado.
- **Ligação**: desktop e CLI criam um `AgentsRevision` por turno e o entregam às duas tools. CLI sem `config_path` usa o
  builder antigo (sem `manage_agents` também, então não há o que atualizar). O texto da tool e a mensagem de retorno
  agora dizem que o agente já aparece no `delegate_to_agent`; **como agente da conversa** (seletor) continua a partir
  da próxima mensagem — o agente ativo é escolhido pelo usuário, não pela tool.
- **Verificação**: 6 testes novos no `warden-core` (5 da tool viva — lista, `call`, resolver sem resultado, uma
  resolução por mudança, orçamento — e 1 do orquestrador); no `warden-bootstrap`, um turno completo em que o chefe cria
  `poet` e delega a ele no mesmo turno (com **teste de mutação**: sem o `bump()` falha com `poet not offered`) e o teste
  da revisão. Binário real do CLI num pty contra um servidor de modelo **falso** (4 checagens): o `enum` de
  `agent_id` passa de `["chief"]` para `["chief","poet"]` na requisição seguinte à criação, o alvo recebe só as tools
  de leitura (sem `delegate_to_agent`/`manage_agents`/`write_file`) e o `poet` fica salvo em disco. **Não feito**:
  modelo real (sem chave), app Tauri aberto de verdade.

## Fila de jobs em segundo plano (P46, Sessão 85)

Até a Sessão 84 o chefe delegava e **esperava**: cada `delegate_task`/`delegate_to_agent` bloqueava o turno até o
sub-agente responder, então três tarefas independentes rodavam em série. Agora a chamada aceita `background: true`.

- **`JobBoard`** (`warden-core/src/jobs.rs`): os jobs de **um turno**. `spawn` devolve o id (`job-1`, `job-2`…) na hora;
  o trabalho roda numa `tokio::spawn` que primeiro pega um slot de um `Semaphore` (`max_parallel`, no mínimo 1). O
  estado (`Queued → Running → Done|Failed`) vive num `watch` — `wait` dorme até terminar e um job que morreu por
  pânico volta como `Failed`, nunca como travamento. **Nada é persistido**: o quadro nasce e morre com o turno.
- **`JobsGuard`**: quem é dono do turno segura o guard; ao cair (turno terminou, deu erro ou o usuário cancelou e o
  future foi largado) chama `abort_unfinished`. Nada sobrevive ao turno — um job que o chefe não coletou é cancelado,
  e a descrição do `background` avisa isso ao modelo.
- **Tool `jobs`** (`list` / `result`, com `wait` por padrão `true`): registrada uma vez no `bootstrap()` **sem quadro**
  e escondida do modelo (`is_available` falso). O `Orchestrator` liga uma cópia ao quadro de cada turno.
- **`Tool::with_jobs(&Arc<JobBoard>)`** (método novo do trait, padrão `None`): é como a tool de delegação e a `jobs`
  recebem o quadro. Só depois de ligada a delegação inclui `background` na spec — nada anuncia o que não funciona.
  `attach_jobs` roda **só na raiz do turno** e só se a `jobs` estiver entre as tools: um agente cujo `allowed_tools` a
  deixou de fora não pode iniciar jobs que não consegue coletar, e sub-agentes nunca veem `jobs` nem `background`
  (não há jobs aninhados).
- **Teto de paralelismo** `max_parallel_jobs` (`config.toml`, `WARDEN_MAX_PARALLEL_JOBS` vence; padrão **3**; `0` =
  um de cada vez, **não** desliga). Baixo de propósito: cada job é uma conversa inteira de modelo e os provedores
  limitam concorrência. Sem UI, mesma postura de `delegate_max_depth`; o desktop preserva o valor ao salvar.
- **Custo**: o job gasta do mesmo `TurnBudget` (`max_delegated_calls`) — background não é jeito de furar o teto. Um
  argumento inválido (agente inexistente) falha **na chamada**, não vira job que falha depois.
- **MCP server**: `warden-mcp-server` passou a filtrar `is_available()` antes de anunciar as tools — sem isso a
  `jobs`, que só existe dentro de um turno com jobs, apareceria pra clientes MCP.
- **Verificação**: testes em `jobs.rs` (limite de concorrência, espera, cancelamento), `job_tools.rs`, na delegação
  (`background` só com quadro ligado) e no orquestrador (três jobs em paralelo coletados, `jobs` ausente ⇒ sem
  `background`, sub-agente nunca vê jobs, job não coletado cancelado ao fim do turno, orçamento compartilhado).
  **Não feito**: modelo real decidindo sozinho paralelizar, binário real no pty, app Tauri aberto.


---

## Limites de gasto por janela de tempo (P4, Sessão 86)

Até a Sessão 85 só existia o teto **por turno** dos sub-agentes (`TurnBudget`). Nada limitava o gasto ao longo do
tempo: um agente em loop ou uma conversa longa gastava até o provedor cortar. Agora há um teto em **tokens e/ou
dólares por janela deslizante**, por escopo, checado **antes de cada chamada de modelo**.

- **`warden-core/src/spend.rs`** (puro, sem modelo nem humano): `Limit` (`id`, `Scope`, `window_hours`, `max_tokens`,
  `max_cost_usd`, `warn_at` = 0.8, `extend_step` = 0.25), `PriceTable`, o **ledger** (`SpendStore` → `FileStore` em
  JSON-lines com um `write` por linha, ou `MemoryStore`), `SpendGuard` (`check`, `record`, `extend`, `status`) e
  `LimitStatus`. Um limite é **a soma do ledger na janela** — não há contador em memória, então desktop, CLI e o bot do
  Telegram (processos separados) gastam da mesma conta se apontam pro mesmo arquivo. A janela **desliza** (últimas N
  horas, não "desde meia-noite"): um de 1h pega loop, um de 24h é o orçamento diário, e os dois valem ao mesmo tempo.
- **Escopos empilhados**: `global`, `agent`, `channel` (`desktop`/`cli`/`telegram`/`whatsapp`/`server`) e `user`
  (`canal:usuário`). Vale o **mais gasto** entre os que se aplicam ao turno (`Check.exceeded`). O gasto de um
  sub-agente conta no contexto da **raiz** (canal/usuário/agente do turno): quem causou o custo é o chefe.
- **Preço**: só o que o usuário cadastra em `[[prices]]` (por milhão de tokens, entrada e saída, chave = id exato do
  modelo). Nada embutido — preço envelhece e um `$` errado com cara de certo é pior que nenhum. Modelo sem preço entra
  nos limites de tokens, e `unpriced_calls` avisa que o `$` está subestimado. Pra isso `ModelProvider::model_id()`
  (padrão `""`) passou a existir e os três providers o implementam.
- **Onde é checado**: no `Orchestrator`, dentro do laço de cada turno (`run_turn`), **antes de cada chamada**, e o uso é
  gravado **depois de cada chamada** — um loop é parado no meio do turno, não depois dele. Cobre todos os canais e
  sub-agentes. O contexto viaja no **`TurnBudget`** (`SpendTurn`), porque ele já é a única coisa que a raiz e cada
  sub-agente (inclusive `delegate_to_agent` e jobs) compartilham — sem segundo canal de propagação. O `TurnBudget`
  agora é criado quando há teto de sub-agentes **ou** limites (`max_calls: Option<u32>`).
- **Pausa em vez de morte** (`SpendTurn::gate`): ao esgotar, o turno **pergunta** pelo `Approver` que o canal já tem
  (desktop, CLI): "sim" concede um `Grant` de um passo (`extend_step` × teto, ex. 25%) **só pelo resto da janela** e o
  turno continua de onde parou; "não"/parar encerra com `SpendLimitReached` (erro tipado, `downcast_ref`). Canal sem
  approver (Telegram, WhatsApp, server, modo por pipe) **bloqueia** com a mensagem — `chat_error_reply` diz o motivo
  sem números. Um `tokio::Mutex` deixa **uma pergunta por vez**: jobs em paralelo batendo no mesmo teto compartilham a
  resposta. Como o limite é da janela e não da conversa, **abrir outra conversa não zera nada** — só estender.
- **O agente enxerga o medidor**: a tool **`budget`** (só leitura, ligada ao turno por `Tool::with_budget`, escondida se
  não há limites; entrou em `SAFE_AGENT_TOOLS`) mostra usado/restante em tokens e $, e quando volta a liberar; e a
  partir de `warn_at` o orquestrador **injeta uma mensagem de sistema só naquela chamada** ("você está perto do limite,
  prefira concluir…"). Longe do teto o custo de contexto é zero.
- **Config** (`warden-bootstrap/src/spend.rs`): `[[limits]]` (`id`, `scope`, `target`, `window_hours`, `max_tokens`,
  `max_cost_usd`, `warn_at`, `extend_step`) e `[[prices]]`. **Sem `[[limits]]` = rede de segurança padrão** (global:
  500k tokens/1h e 2M/24h — folgada pro uso normal, pega loop na primeira hora); `limits = []` ou
  `WARDEN_SPEND_LIMITS=off` desliga tudo. Entrada inválida é **pulada com aviso** e não derruba a inicialização (o app
  é o que deixaria o usuário consertar). `WARDEN_SPEND_LEDGER` troca o arquivo (padrão
  `<config>/warden/spend_ledger.jsonl`, podado na abertura ao tamanho da janela mais longa). Desde a Sessão 88 o
  desktop **edita** `limits`/`prices` na tela de Settings (seção "Spending limits", ver abaixo).
- **Falha do ledger não trava o app**: leitura ilegível = vazio; escrita que falha fica em `last_error`, mostrada por
  `/limits` e pela tool `budget` ("os números podem estar baixos").
- **CLI**: `/limits` (todos os limites, não só os do terminal) e `/extend <id>`; o canal `cli` é fixado em `main.rs`
  pros dois modos. `/e`/`/ex` deixaram de completar sozinhos pra `exit` (agora há `/extend`). **Desktop**: só o
  `ApprovalModal` ("Spending limit reached", botões Stop/Allow more) — a tela de limites/preços veio na Sessão 88.
- **`$` no `/usage` do CLI (Sessão 87)**: `SpendGuard::spent_since(canal, desde)` soma o ledger, cada chamada com o preço
  do **modelo em que rodou** (um sub-agente pode usar outro; `usage_total × preço do turno` daria um número que parece
  certo e não é). Sem preço = "indisponível"/"$X ou mais", nunca zero; sem limites ativos não há ledger. Ledger é por
  canal: dois terminais abertos juntos somam um no outro.
- **Tela de limites e preços no desktop (Sessão 88)**: seção "Spending limits" em Settings (`SpendingSection.tsx`;
  backend em `desktop/src-tauri/src/spend_cmds.rs`, mesmo molde do `ssh_cmds.rs`). Decisões:
  - `limits` é **`null` (nenhum `[[limits]]` → rede de segurança) ≠ `[]` (tudo desligado)** de ponta a ponta — snapshot,
    payload de save e TS. A tela mostra a rede como cartão ("Customize limits" copia os padrões pra cartões editáveis,
    "Turn all limits off" grava `[]`, "Restore built-in safety net" volta a `null`). Salvar sem mexer **não** grava os
    padrões no arquivo (senão mudar o padrão no código nunca mais alcançaria quem só abriu a tela). Os números 500k/2M
    vêm do backend (`default_limit_configs`), não são repetidos no frontend.
  - `limits`/`prices` no payload de save **não têm `#[serde(default)]`**: um formulário que esqueça de mandá-los tem que
    falhar, não virar "sem limites" e trocar em silêncio os limites do usuário pela rede padrão.
  - No **save** uma entrada inválida é **recusada** (a pessoa está ali e conserta), com as mesmas regras do startup
    (`LimitConfig::to_limit`); no **startup** segue "pula com aviso". Nome de limite repetido, preço vazio/negativo/
    `NaN`/repetido são recusados. Um preço `0`/`0` é válido (modelo local grátis).
  - **Não** se recusa limite de agente que já não existe: o agente pode ter sido apagado pela tool `manage_agents` ou nesta
    mesma tela, e recusar todo save por causa de sobra seria a armadilha de referência pendurada dos hosts SSH (Sessão 83).
    O cartão mostra "(no longer exists)".
  - `WARDEN_SPEND_LIMITS=off` no ambiente vence o arquivo; o snapshot traz `limitsDisabledByEnv` e a tela avisa.
  - Campos numéricos guardam o **texto digitado** (rascunho) e só sobem o número parseado — senão `0.` vira `0` a cada
    tecla. `warnAt`/`extendStep` aparecem em % (0,8 ↔ 80) e vão pro backend como fração. Cada limite ganha uma frase em
    linguagem comum ("The agent "pirate": pauses at $0.5 within any 6 hours.").
  - `validateSpending` (TS) espelha o backend só pra avisar antes do IPC; o backend continua a autoridade.
- **Decisões deixadas de fora de propósito**: sem tabela de preço embutida; sem teto por turno (o loop já é parado pela
  janela de 1h); Telegram/WhatsApp contam por **chat** (`chat.id`, não o id do remetente — num grupo, o grupo inteiro);
  a extensão não é permanente (expira com a janela).
- **Limitação aceita**: o teto é **por chamada**, não por token — uma chamada grande que começa abaixo do teto pode
  ultrapassá-lo (o excesso é gasto e contado; a próxima chamada é que é barrada).
- **Wizard de limites e preços no CLI (Sessão 89)**: `/limits add`/`edit <id>`/`remove <id>`/`off`/`reset` e
  `/prices`/`add`/`edit <model>`/`remove <model>`, mesmo molde do wizard de SSH (`prompt_limit`/`prompt_price`
  montam a struct campo a campo, validada de verdade por `LimitConfig::to_limit()`). `/limits add`/`edit` sobre um
  `config.limits` em `None` materializam `default_limit_configs()` antes de aplicar a mudança — um `/limits add`
  nunca desliga a rede padrão sem querer; `off`/`reset` (os dois que perdem proteção de verdade) e `remove` pedem
  confirmação. `CONFIG_RESTART_NOTE` (ex-`SSH_RESTART_NOTE`, renomeada — texto já era genérico) documenta em toda
  confirmação a mesma limitação que a tela do desktop já tinha: `SpendGuard` é montado uma vez em `bootstrap()`, sem
  recarga em quente — uma edição só vale no próximo `warden` iniciado, `/limits`/`/prices` (leitura) continuam
  mostrando o estado **congelado no boot**, nunca a própria edição da sessão atual.

## Sync seletivo do vault: `.syncignore` (P75, Sessão 92)

Até aqui `diff::diff_vault` (`crates/warden-sync`) varria `Vault::list_all_files()` sem filtro nenhum — o motor de
sync (tanto o backend Arweave/TruthID quanto o `GitSyncEngine` irmão do P63) sempre espelhava o vault inteiro entre
devices. O usuário pediu uma forma de marcar parte do vault como "fica só neste device".

- **`.syncignore` na raiz do vault**, um padrão glob por linha (`#` comenta, linha em branco ignorada) — estilo
  `.gitignore` simplificado: sem `/` casa em qualquer profundidade (`secret.md` casa `notes/secret.md` também);
  `pasta/` casa a pasta inteira; `/arquivo.md` ancora na raiz. Sem negação (`!padrão`) — não pedido. Módulo novo
  `crates/warden-sync/src/syncignore.rs` (`SyncIgnore`, via `globset` — mesma lib do ripgrep, sem puxar o `ignore`
  crate inteiro que também caminha diretório, coisa que `Vault::list_all_files` já faz). **Dot-prefixed de propósito**:
  `Vault::collect_all_files` (`crates/warden-core/src/memory/mod.rs`) já pula qualquer entrada cujo nome comece com
  `.` (mesma regra que esconde `.git`/`.DS_Store`), então o próprio `.syncignore` nunca aparece no diff — resolve de
  graça o problema recursivo "o arquivo de exclusão precisa decidir se ele mesmo sincroniza".
- **Regra uniforme, sem exceção** (decisão explícita do usuário ao aprovar o plano): um padrão que bate nos 3
  arquivos fixos (`_profile.md`/`_behavior.md`/`_feedback.md`) ou em `skills/` também os exclui — mais simples e mais
  flexível que dar imunidade especial a eles; quem não quiser isso simplesmente não escreve um padrão que bata.
- **Efeito nas duas direções, sem tocar nas assinaturas públicas**: `diff_vault` e `bundle::apply_bundle` já recebem
  `&Vault` em toda chamada existente (`push.rs`, `pull.rs`, `lib.rs::status`, `git.rs` push/pull) — carregar
  `SyncIgnore::load(vault)` **dentro** das duas funções faz o recurso valer pros dois motores de sync de graça, sem
  mudar nenhum call site.
  - *Push*: `diff_vault` pula um path ignorado tanto no loop de `added_or_modified` quanto no filtro de `deleted` —
    crítico este segundo: sem isso, ligar um `.syncignore` pra um arquivo **já sincronizado antes** faria o próximo
    push reportá-lo como deletado e apagá-lo dos outros devices. Com o filtro, o arquivo simplesmente some do diff
    nas duas direções; o hash antigo em `manifest.vault_files` fica inerte enquanto o padrão continuar batendo (se o
    padrão for removido depois, o diff volta a examiná-lo normalmente contra esse hash antigo).
  - *Pull*: `apply_bundle` pula escrita/deleção de qualquer path que bata no `.syncignore` **do vault de destino** —
    um bundle de outro device que não tem essa regra (ou teve o arquivo sincronizado antes dela existir) nunca toca
    esse path aqui. `ApplyReport` ganhou `files_ignored: usize`; o loop de aviso "mudança local foi sobrescrita pelo
    pull" (`pull.rs`, `git.rs::pull`) também pula esses paths, senão avisaria sobre uma sobrescrita que não
    aconteceu. `PullOutcome`/`GitPullOutcome` ganharam o mesmo campo, propagado até `/sync pull`/`/sync git pull` no
    CLI e o card de resultado no desktop (`SyncView.tsx`).
  - `SyncStatus` ganhou `syncignore_pattern_count` (visível em `/sync` no CLI e no card de status do desktop) — só a
    contagem, não os padrões em si.
- **Sem comando novo**: `.syncignore` é um arquivo de vault normal, editável com qualquer ferramenta de escrita já
  existente (inclusive pelo próprio agente via `write_vault_file`) — não precisa de wizard dedicado.
- **`config.toml` continua fora disso, de propósito**: o arquivo inteiro (chaves de API, agentes, SSH, limites de
  gasto) segue sincronizando sempre, sem seleção por campo — comportamento antigo e intencional (P37, Sessão 50),
  não uma regressão. Sync seletivo de `config.toml` ficou fora de escopo (mudaria a granularidade de "arquivo" pra
  "campo", problema mais difícil que ninguém pediu ainda).

## Grupo de abas na extensão de navegador (P69 item 1, Sessão 92)

As 4 tools de DOM (`browser_read_page`/`click_element`/`navigate`/`extract_text`, Fase 8.3-8.6) operavam só na aba
ativa no momento da chamada, via `activeTab` — permissão que só libera acesso à aba em foco no instante de um gesto
do usuário (clicar no ícone, abrir o painel) e cai quando essa aba navega. O usuário pediu replicar o modelo do
Claude no Chrome: um grupo de abas dedicado onde a IA age em qualquer aba do grupo, não só a ativa.

- **Modelo de permissão confirmado com o usuário antes de codar**: o usuário adiciona aba por aba, não a IA. Um
  botão "+ Adicionar esta aba" no painel lateral (`TabsView.tsx`) é o próprio gesto que concede `activeTab` pra
  aquela aba específica — a mesma permissão de sempre, só que acumulada por aba em vez de reposta a cada gesto.
  Nenhuma permissão nova no manifest além de `tabGroups` (API de agrupamento visual — `chrome.tabs.group`/
  `chrome.tabGroups.update`), que por si só não amplia acesso a conteúdo nenhum. Mantém a postura "mínimo possível"
  já documentada em `manifest.config.ts` (a nota sobre evitar `host_permissions: ["<all_urls>"]` continua valendo,
  agora com uma frase a mais explicando por que `tabGroups` não é uma exceção a essa regra). A IA nunca abre nem
  adiciona uma aba ao grupo sozinha — só enxerga e age nas que o usuário já adicionou.
- **`extension/src/background/tab_group.ts`** novo — estado inteiramente em memória (`grantedTabs: Set<number>` +
  `groupId`), mesma postura de `history`/`connection` em `index.ts`: morre com o service worker, sem persistência,
  sem modo de falha novo. `addActiveTabToGroup()` resolve a aba ativa (mesma query que `dom_executor.ts` já fazia),
  cria o grupo do Chrome na primeira adição e reaproveita depois; `removeTabFromGroup(tabId)` desagrupa e tolera a
  aba já ter fechado; `isTabInGroup(tabId)` — usada por `dom_executor.ts` pra validar um `tabId` explícito antes de
  tentar `executeScript` nele, com erro claro em vez de deixar a falha genérica do Chrome estourar; `listGroupTabs()`
  lê `title`/`url` de cada aba do grupo **sem precisar da permissão `tabs`** — o `activeTab` concedido no gesto de
  adicionar já libera ler esses campos daquela aba específica, então a lista funciona dentro do mesmo orçamento de
  permissão. `chrome.tabs.onRemoved` poda o set proativamente quando uma aba do grupo fecha.
- **`dom_executor.ts`**: `getActiveTabId` virou `resolveTabId(explicitTabId?)` — com um `tabId` explícito, valida
  contra `isTabInGroup` e o usa; sem ele, cai no comportamento de sempre (aba ativa), **zero mudança pra quem nunca
  abre a aba "Abas" do painel**. `runInPage`/`navigateActiveTab` ganharam esse parâmetro opcional a mais, e a
  mensagem de erro de falta de acesso agora distingue os dois casos (aba explícita: "remova e adicione de novo";
  aba implícita: "abra o painel lateral nessa aba").
- **As 4 tools existentes ganharam um `tabId` opcional** no JSON Schema (`parseOptionalTabId`, helper compartilhado
  em `dom_executor.ts` — mesmo padrão de parsing solto que cada tool já fazia pro próprio campo). **Tool nova
  `browser_list_tabs`** — sem parâmetros, devolve a lista do grupo; é como a IA descobre quais `tabId` são válidos
  antes de passar um pras outras 4.
- **UI**: terceira aba "Abas" em `App.tsx` (ao lado de Chat/Skills, mesmo padrão de mount-sempre-mas-`hidden`/
  renderiza-só-quando-ativa que as outras duas já usam). `TabsView.tsx` reaproveita as classes CSS de `SkillsView`
  (`.skills-hint`/`.skills-list`/`.skills-item`/etc., estendidas com seletores `.tabs-*` irmãos em `App.css` em vez
  de duplicar as regras) — lista as abas do grupo, botão de adicionar a atual, botão de remover por linha. Um
  evento novo `groupChanged` (mesmo broadcast de `statusChanged`/`chatMessage` já existente) mantém a lista
  atualizada quando uma aba do grupo fecha sem nenhuma ação do painel ter disparado isso.
- **Verificação**: `npx tsc --noEmit`/`npm run build` limpos dentro de `extension/` (sem framework de teste no
  projeto — mesma lacuna estrutural já registrada em P67/P68), `dist/manifest.json` conferido com `tabGroups` na
  lista. **Não verificado de ponta a ponta contra um Chrome real** (carregar `extension/dist`, adicionar 2+ abas,
  pedir pra IA agir numa que não é a ativa) — mesma lacuna aceita de sempre neste ambiente, sem browser interativo
  disponível; fica pro usuário testar manualmente fora daqui, mesmo padrão que fechou a verificação original da
  Fase 8.1/8.2 (Sessão 68).
- **Fora deste plano, de propósito**: Firefox (a segunda metade do P69, decisão separada — `chrome.sidePanel`/
  `chrome.tabGroups` não têm equivalente direto lá); a IA abrir/adicionar abas novas sozinha (fora do modelo de
  permissão escolhido); persistir o grupo entre reinícios do service worker (mesma postura efêmera do resto do
  estado em `index.ts`).

## MarkdownV2 nas respostas do Telegram (P19, Sessão 92)

`TelegramClient::send_message` (`crates/warden-telegram/src/telegram.rs`) mandava a resposta do modelo como texto
cru — decisão explícita da Fase 2 porque o MarkdownV2 do Telegram tem sintaxe própria (diferente de CommonMark) e a
API rejeita a mensagem **inteira** se um caractere reservado não for escapado direito. O desktop já renderiza a
mesma resposta CommonMark via `react-markdown`+`remark-gfm`; faltava o conversor de verdade pro Telegram.

- **`pulldown-cmark` como dependência nova** (`crates/warden-telegram/Cargo.toml`) — parser CommonMark real,
  orientado a eventos, em vez de regex (regex quebra em qualquer aninhamento: negrito dentro de item de lista,
  código dentro de link). Único crate de Markdown no workspace inteiro até aqui. Só `ENABLE_STRIKETHROUGH`
  habilitada, pareando com o `remark-gfm` do desktop — **deliberadamente sem** `ENABLE_TABLES`/`ENABLE_TASKLISTS`:
  sem essas extensões o parser trata essa sintaxe como texto de parágrafo comum, que o escape de texto solto já
  degrada pra algo legível, sem exigir nenhum código de tabela dedicado (o Telegram não tem como renderizar uma
  mesmo).
- **`crates/warden-telegram/src/markdown_v2.rs::to_markdown_v2`** — percorre os `Event`s do parser: negrito/
  itálico/tachado viram `*`/`_`/`~` (mapeamento direto Start/End, válido mesmo aninhado — MarkdownV2 aceita
  `_*negrito itálico*_`); heading vira uma linha em negrito (sem equivalente no Telegram); listas viram linhas
  prefixadas com `• ` (não-ordenada) ou `N\. ` (ordenada, contador de `Tag::List(Some(start))`) — bullet Unicode em
  vez de `-` literal pra não precisar escapar o marcador; link vira `[texto](url)`, com o texto passando pelo
  escape de texto solto e a URL por um escape próprio (só `)`/`\`, regra específica do Telegram pro parêntese de um
  link). Texto solto escapa os 18 caracteres reservados do MarkdownV2 (`_*[]()~\`>#+-=|{}.!\`, a barra invertida
  inclusa nessa lista).
- **Achado real ao codar, corrigido antes de fechar**: o conteúdo de um bloco de código chega como `Event::Text`
  comum (não `Event::Code`, que é só pra spans inline) — sem tratar isso, `(`/`)`/`.` etc. dentro de um bloco de
  código viravam `\(`/`\)`/`\.`, quebrando a formatação (código com parênteses é praticamente garantido). Corrigido
  com uma flag `in_code_block` que roteia `Event::Text` pro `escape_code` (só `` ` ``/`\`, a regra mais permissiva
  que o Telegram usa dentro de `code`/`pre`) em vez do `escape_text` normal enquanto dentro de um bloco.
- **Blockquote (`>` por linha)**: como o texto flui incrementalmente por um `String` só, e o conteúdo de dentro de
  um blockquote pode ter suas próprias entidades/quebras de linha, o marcador de abertura (`Tag::BlockQuote`)
  grava o índice (`out.len()`) onde o conteúdo começou; o de fechamento usa `String::split_off` pra recortar tudo
  que foi emitido desde ali, reemitindo linha por linha com `>` na frente.
- **`TelegramClient::send_message`** — fatorado em `send_message` (assinatura pública de sempre) + `send_one`
  (privado, um `sendMessage` com `parse_mode` opcional). Converte a resposta inteira; se coube num chunk só
  (`<= TELEGRAM_MESSAGE_LIMIT`), tenta mandar formatada — se o Telegram rejeitar (`ok: false`, o cenário que
  motivou a decisão original de não fazer isso), loga em stderr e reenvia o **texto original sem formatação** como
  fallback, nunca perdendo a resposta por causa de um bug de escape. **Réplicas longas o bastante pra precisar de
  mais de um chunk continuam em texto puro, decisão deliberada, não lacuna**: não há garantia de que o texto
  convertido e o texto puro cortariam nos mesmos pontos de byte, e um fallback por chunk arriscaria reenviar um
  chunk já bem-sucedido duas vezes se um chunk posterior falhasse — restringir ao caso de chunk único elimina o
  risco por completo; como a maioria das respostas cabe num chunk só, isso cobre o caso comum sem regressão no raro
  (a resposta longa simplesmente perde a formatação, exatamente como sempre foi).
- **Fronteira de teste inalterada**: a lógica de fallback vive dentro de `TelegramClient` (a implementação HTTP
  real), não no trait `TelegramApi` mockado pelos testes de `process_updates`/`handle_update` já existentes — não
  ganhou cobertura nova (mesma lacuna que já existia pra `TelegramClient` antes deste plano, o crate não tem
  mock de HTTP). O `to_markdown_v2` puro, testável sem rede, é quem ganhou os 11 testes novos.
- **Verificação**: `cargo test -p warden-telegram` (22, 11 novos) e `cargo test --workspace`/`cargo clippy
  --workspace --all-targets` limpos. Sem teste de ponta a ponta contra o Bot API real (sem token/chat de Telegram
  disponível neste ambiente) — mesma lacuna aceita de sempre pra esse canal.
- **Fora de escopo, documentado**: tabelas do GFM (degradam pra texto escapado); chunking "esperto" que preserva
  entidades através de múltiplas mensagens; spoilers (`||texto||`, sem equivalente em CommonMark).

## Interface web servida pelo próprio hub (P78 fatia 1, Sessão 98)

- **Decisões do usuário** (debate da Sessão 98): frontend **novo** em `web/` (React 19 + Vite + TS), não o React
  do desktop; a fatia 1 só usa o que o protocolo do hub já tem (chat com histórico e skills); o navegador se
  autentica **como mais um device** — chave de pareamento uma vez, depois o token por device do P36 guardado no
  `localStorage`, revogável pela lista de devices que já existe. Nenhuma autenticação nova no servidor.
- **Mesma porta do WebSocket.** Página e WS ficam na mesma origem (`ws(s)://location.host`): sem CORS, sem
  conteúdo misto, e o TLS do Tailscale (P36) cobre os dois. `route_connection` (`server.rs`) agora lê o cabeçalho
  HTTP antes do tungstenite (`web_ui::read_request_head`, `httparse`, limite de 8 KiB e 10s); com
  `Upgrade: websocket` segue o fluxo de antes, devolvendo os bytes já lidos via `web_ui::Rewind`, e sem upgrade
  responde um arquivo e fecha. Num hub só-TLS, HTTP puro recebe `308` para o `https://` quando o hub conhece o
  próprio nome (`secure_url`), senão `426`; a descoberta por `ws://` (upgrade em `DISCOVER_PATH`) continua igual.
- **Servidor HTTP escrito à mão, só leitura**: `GET`/`HEAD` (o resto recebe `405`), fallback de SPA (caminho sem
  extensão cai no `index.html`), recusa de `..`/`.`/segmento vazio/`\` (o `rust-embed` em debug lê do disco),
  `Cache-Control` imutável em `assets/*` (nomes com hash do Vite) e `no-cache` no resto, `nosniff`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Connection: close`. Um framework HTTP inteiro seria
  desproporcional para servir arquivos estáticos numa porta que já é do tungstenite.
- **Assets embutidos no binário** (`web_ui::EmbeddedWebUi`, `rust-embed` com `allow_missing`): um checkout sem
  `npm run build` compila e passa nos testes, e a página vira um `503` explicando o que fazer. Um `build.rs` no
  `warden-server` avisa o cargo quando o `web/dist` muda (ou, enquanto ele não existe, quando `web/` muda),
  porque o `rust-embed` sozinho não percebe arquivos novos. Sem isso, `npm run build` seguido de `cargo build`
  não embutia a página (achado ao testar). O desktop embute o mesmo `web/dist`, sem mexer nos recursos do Tauri.
- **Opt-in no `Server`** (`with_web_ui(Arc<dyn WebAssets>)`, no padrão do `with_tls`): sem ele o hub continua só
  WebSocket, então os testes antigos não mudaram. O `warden-server` liga por padrão (`--no-web-ui` desliga) e o
  hub embutido do desktop liga sempre, com o endereço no status (`webUrl`) e um link na tela Workspace.
- **Cliente**: `web/src/hub/messages.ts` é cópia do `extension/src/protocol/messages.ts` (manter em sincronia,
  como o espelho em Dart do mobile), e `web/src/hub/connection.ts` é uma adaptação do `connection.ts` da
  extensão: URL inteira em vez de host/porta, nenhuma tool local, anexos nas entradas do chat, `HandshakeError`
  com `authRejected`. A reconexão com backoff (1s até 30s) fica no `App.tsx`. O `deviceId` usa
  `crypto.getRandomValues`, porque o `randomUUID` só existe em origem segura e um hub de LAN costuma ser `http://`.
- **Limite herdado, não da web**: um turno que falha (ex.: chave de API inválida) não é gravado na conversa
  (`handle_turn` sai no `?` antes do `save_conversation`), então esse `chatError` some do histórico ao
  recarregar a página, igual acontece no mobile e na extensão.

## Várias conversas por device no hub (P78, Sessão 99)

- **Decisões do usuário**: as conversas continuam **por device** (cada device só vê as suas; compartilhar entre
  devices fica para depois). A fatia cobre listar, criar, trocar, renomear e apagar, na web, na extensão e no
  mobile.
- **No disco**: uma pasta por device, `conversations-server/<device>/<conversa>.json`, lida e escrita pelas mesmas
  funções do `warden-bootstrap`. O arquivo de antes do P78 (`conversations-server/<device_id>.json`) é movido
  para dentro dela como a conversa `default` uma vez, quando o device conecta (`device_conversations_dir`), antes
  de qualquer pedido dele. Um arquivo antigo corrompido é movido como está, então o `RequestHistory` continua
  mostrando o erro de parse.
- **Ids validados** (`conversations::is_valid_id`: 1 a 64 caracteres de `[A-Za-z0-9_-]`). Um `conversationId`
  fora disso é recusado antes de chegar ao disco. Um `device_id` fora disso (o `warden-node --device-id` aceita
  qualquer texto) ganha uma pasta com nome de hash, porque antes o `device_id` virava nome de arquivo sem
  checagem nenhuma.
- **Protocolo aditivo**: `Chat.conversationId?` e `RequestHistory.conversationId?`, onde ausente significa a
  conversa `default`; assim, um cliente de antes do P78 (ou o `warden-node`) continua funcionando sem mudar. O
  `ChatResponse`/`ChatError` devolvem o `conversationId`: o `Chat` não tem `requestId`, e sem isso uma resposta
  atrasada cairia na conversa aberta no momento. Mensagens novas: `ListConversations` → `ConversationList`, e
  `RenameConversation`/`DeleteConversation` → `ConversationOk`/`ConversationError`.
- **A conversa nasce no primeiro `Chat`**: o cliente gera o id e o hub cria o arquivo, com o título tirado da
  mensagem (`title_from`). Até a resposta chegar ela ainda não está no disco, então os clientes a mostram na
  lista de forma otimista e bloqueiam renomear/apagar enquanto há turno pendente naquela conversa.
- **Concorrência no `handle_turn`**: a chamada do modelo leva até um minuto, então agora o arquivo é relido
  depois dela, sob um `Mutex` do processo (`CONVERSATION_WRITES`) que o `rename_conversation`/
  `delete_conversation` também usam. Assim, uma renomeação feita durante o turno não é sobrescrita, e uma
  conversa apagada durante o turno não volta (a resposta ainda é entregue, mas não é gravada). O lock só
  protege I/O de arquivo, nunca a chamada do modelo. Renomear não mexe no `updated_at`, para não reordenar a lista.
- **Espera por conversa nos clientes**: cada conversa aguarda a própria resposta, e dá para mandar mensagem em
  outra enquanto isso. Como o hub só grava o turno depois de respondido, ao voltar para uma conversa pendente o
  cliente mostra o histórico mais a pergunta que ainda espera. A última conversa aberta é lembrada:
  `localStorage` na web, `chrome.storage` na extensão e `SharedPreferences` por hub no mobile.
- **UI**: na web, barra lateral (gaveta no celular); na extensão, um `<select>` com ações, porque o painel é
  estreito; no mobile, um `endDrawer`, para não tirar a seta de voltar do `ChatScreen`. No mobile o
  `ChatTranscript` recebe uma `ConversationBackend` (que o `ServerConnection` implementa) em vez de funções
  soltas, e por isso dá para testá-lo com um fake.

## Anexos e voz enviados do navegador (P78, Sessão 99, continuação)

- **Decisões do usuário**: imagens, voz, arquivos de texto e PDF; imagens reduzidas no navegador; só a web
  nesta fatia (o protocolo já serve para mobile e extensão depois).
- **PDF entra no núcleo, sem Files API**: os três provedores agora aceitam PDF inline em base64. Na Anthropic,
  bloco `document`; na OpenAI, parte `file` com `file_data` em data URL (o nome é fixo, `attachment.pdf`,
  porque o `Attachment` não carrega nome); no Gemini, o mesmo `inlineData` da imagem. Isso revê a decisão da
  Sessão 40 ("OpenAI exige upload separado"), que ficou desatualizada. `USER_ATTACHMENT_MIME_TYPES` em
  `warden_core::model` é a lista aceita. Junto, uma correção latente: uma mensagem só com anexo não manda mais
  bloco de texto vazio, que a Anthropic recusa.
- **Protocolo**: `Chat.attachments` (aditivo, vazio por padrão) e `Transcribe` → `Transcription`/
  `TranscriptionError`. O `handle_turn` recebe os anexos e os grava no turno do usuário, então o histórico os
  mostra e os turnos seguintes os reenviam ao modelo, como no desktop (custo: um PDF volta em todo turno
  daquela conversa).
- **Validação no hub** (`chat_input.rs`): só os tipos da lista, no máximo 10 por turno e **12 MiB de base64 no
  total**. O navegador manda a mensagem inteira num frame só, e o tungstenite lê frames de até 16 MiB, então um
  anexo maior derrubaria a conexão em vez de dar erro. A web confere o mesmo limite antes de enviar. Um turno
  sem texto recebe o título "Image" ou "Document".
- **Texto vai no próprio texto da mensagem**: arquivos de texto (até 200 KB) são lidos no navegador e entram na
  mensagem como bloco cercado, com o nome do arquivo. A cerca é maior que qualquer sequência de crases do
  arquivo. Não precisou mudar provedor nenhum.
- **Imagens**: redimensionadas para no máximo 2048 px e recomprimidas em JPEG 0,85 com fundo branco, a menos
  que já sejam pequenas (até 1 MB e dentro dos 2048 px). GIF vai como está, para manter a animação.
- **Voz**: `MediaRecorder` no navegador (webm/opus no Chrome/Firefox, mp4 no Safari) e o hub transcreve com o
  Whisper. A chave é a mesma `api_keys.whisper` do microfone do desktop, relida do config a cada chamada
  (`WhisperTranscriber`, atrás de uma trait `Transcriber` para os testes não chamarem a API). O
  `audio_filename_for_mime_type` saiu do desktop para o `warden_core::transcribe` e passou a ignorar
  parâmetros como `;codecs=opus`. **O navegador só libera o microfone em HTTPS ou `localhost`**: num hub de LAN
  em `http://`, o botão aparece desabilitado com a explicação, e a voz depende do TLS do Tailscale (P36).

## Vault na web, com edição também no desktop (P78, Sessão 100)

- **Decisões do usuário**: ler e editar (criar, editar e apagar notas), com busca, sem a pasta `skills/` na
  árvore (ela tem tela própria), e o mesmo editor levado ao desktop, que até aqui era só leitura (P52).
- **Uma implementação, dois clientes**: as regras moram no `warden_core::memory::notes` (`browse_files`,
  `read_note`, `save_note`, `delete_note`). O hub (`vault.rs`, mensagens `ListVaultFiles`/`ReadVaultNote`/
  `SaveVaultNote`/`DeleteVaultNote`/`SearchVault`) e os comandos Tauri (`vault_cmds.rs`) são só invólucros finos.
- **Versão por conteúdo, não por mtime**: a versão de uma nota é o SHA-256 dos bytes. Salvar manda a versão
  aberta; se a nota mudou (a IA escreveu, o sync puxou, outra tela salvou), a resposta é um conflito
  (`VaultError { conflict: true }` / `NoteConflict`), e a tela oferece "recarregar" ou "sobrescrever". Criar
  (sem versão) recusa um caminho já ocupado. Por conteúdo, reescrever o mesmo texto não gera conflito falso. A
  checagem e a escrita ficam sob um `Mutex` do `Vault`, e a escrita é atômica (arquivo temporário oculto +
  `rename`). A IA escreve pelo `WriteFileTool` sem passar por essa trava: a janela de corrida que sobra é de
  milissegundos e não perde dado de forma silenciosa.
- **Caminhos vindos de fora**: o editor recusa caminho absoluto, `..`, qualquer componente começando com `.`
  (`.warden/`, `.syncignore`), a pasta `skills/` da raiz e qualquer caminho que, resolvido, saia do vault por um
  symlink. Notas até 1 MiB; binário não abre ("not a text file").
- **Correções de segurança que já existiam, achadas no caminho**:
  - `Vault::read`/`write`/`delete` faziam `root.join(path)` sem conferir nada. As tools da IA, o
    `LocalFSProvider` (que atende o `vault_read`/`vault_write` de outro nó, P61) e o `apply_bundle` do sync
    podiam escapar do vault com `../`. Agora passam por `Vault::path_of`, que recusa caminho absoluto e `..`.
  - As listagens (`list_all_files`, `list_files`, e com elas a busca e o sync) seguiam symlinks. Um link para
    fora punha arquivos externos na busca e no sync, e um link para uma pasta acima entrava em recursão até o
    limite do sistema; isso foi visto de verdade no teste ponta a ponta. **Agora symlinks não são seguidos nas
    listagens.** Quem usa symlink para montar pastas dentro do vault (comum no Obsidian) deixa de vê-las no
    Warden.
- **`sha2` deixou de ser opcional no `warden-core`**: a versão das notas precisa dele sem a busca semântica. É
  Rust puro e compila no Android.

## Uso e gasto na web, com limites também no desktop (P78, Sessão 100, continuação)

- **Decisões do usuário**: a tela mostra **o hub inteiro** (todos os devices, limites globais e o ledger de
  todos os canais da máquina), a web **pode liberar** um limite esgotado, e entram o gráfico por dia, o gasto
  recente por modelo/canal e o painel de limites na tela Usage do desktop.
- **Duas fontes, cada uma com o que tem**: os **tokens** vêm das conversas do hub (todas as pastas de device,
  mais o arquivo antigo de quem não reconectou desde o P78), com histórico completo e data por mensagem. O
  "por agente/provedor" do desktop não serve aqui, porque as conversas do hub gravam `agent_id`/`provider_id`
  vazios. Então o recorte é **por device** (nome vindo do `PairingStore`) e **por dia**, no fuso de quem vê
  (`RequestUsage.tzOffsetMinutes`). **Dólar e modelo** só existem no ledger do P4, que guarda apenas a janela do
  limite mais longo e é compartilhado com todos os canais. Daí `SpendGuard::breakdown()` (por modelo e por canal)
  e a tela deixando claro que o "gasto recente" é dessa janela.
- **Protocolo**: `RequestUsage` → `UsageReport` (`UsageReportDto`), `ExtendLimit` → `LimitExtended`, e
  `UsageError`. O `ChatError` ganhou `spendLimitId`: quando o turno para num limite (`SpendLimitReached`), a
  bolha de erro do chat oferece "Liberar mais" direto. A liberação **não reenvia** a mensagem sozinha. É o
  equivalente, para um cliente que não pode ser perguntado no meio do turno, ao que o `Approver` faz no desktop.
- **Qualquer device pareado pode liberar**: o hub é de uma pessoa só e todo device pareado é dela; liberar é o
  mesmo `SpendGuard::extend` (um `extend_step`, só até o fim da janela) que o desktop e a CLI já fazem.
- **`LimitStatusDto::all(guard)`** mora no crate do protocolo e é usado pelo hub e pelo desktop (que passou a
  depender de `warden-server-protocol` direto), então as duas telas recebem o mesmo formato.
- **Datas sem crate de calendário**: `daily_usage` formata `YYYY-MM-DD` com o `civil_from_days` do Howard
  Hinnant, em vez de trazer `chrono` só para isso.
- **Visual** (skill `dataviz`): uma série só no gráfico diário, sem legenda; colunas de até 24 px com ponta
  arredondada e 2 px de espaço; tooltip por coluna no hover/foco e tabela alternativa. Nos medidores, o
  preenchimento muda de destaque para aviso (`#fab219`, só como preenchimento) e depois para perigo, sempre com
  ícone e rótulo.

## Configurações na web, com orquestrador trocável no hub (P78, Sessão 101)

- **Decisões do usuário**: a web edita **provedores, agentes, chaves Tavily/Whisper, limites e preços**. Shell,
  servidores MCP, hosts SSH, armazenamento e caminhos ficam **fora**, porque dariam a qualquer device pareado um
  jeito de executar comandos na máquina do hub. **Ler é livre para qualquer device; salvar pede a chave de
  pareamento de novo**, então um token de device vazado sozinho não troca chaves de API. **Chave nova só por TLS ou
  pela própria máquina** (`settings::is_secure`: conexão TLS ou peer loopback, incluindo `::ffff:127.0.0.1`).
  Remover uma chave e o resto das configurações funcionam em http:// de LAN, porque nenhum segredo trafega.
  **Revisto no P119 (Sessão 121)**: a web passou a editar também o que alcança a máquina, mas atrás de uma trava no hub;
  ver "A web edita a máquina do hub, com trava" mais abaixo.
- **Segredos nunca voltam ao navegador**: a tela recebe `SecretStatusDto { set, hint }` (os 4 últimos caracteres,
  só a partir de 16) e manda `SecretEdit` (`keep`/`set`/`clear`). Um provedor renomeado acha a chave salva pelo
  `originalId`. Agentes também levam `originalId`, para que renomear ou apagar um agente atualize os hosts SSH que
  a tela não mostra (apagar segue o `remove_agent_from`: um host sem agente é desligado, nunca aberto a todos).
- **Uma validação, dois clientes**: `warden_bootstrap::settings` tem `check_providers`/`check_agents`/
  `check_active_provider`/`limits_into_config`/`prices_into_config` e o `apply_hub_settings`. O `save_settings`
  do desktop passou a usar as mesmas funções, e os payloads de limite/preço do `spend_cmds.rs` foram trocados
  pelos DTOs do protocolo (`LimitSettingsDto`/`PriceSettingsDto`).
- **Versão por conteúdo do `config.toml`** (`config_version`, o mesmo SHA-256 das notas do vault), nos dois
  sentidos: a web recusa salvar sobre uma mudança do desktop (ou de alguém editando à mão), e o `save_settings`
  do desktop agora também recusa salvar um formulário aberto antes de uma mudança da web. Antes, ele regravava
  as chaves antigas.
- **Salvar é tudo ou nada** (`handle_save_settings`, serializado por um `Mutex` do hub): chave de pareamento
  (comparação sem atalho e 1 s de espera se errar) → segredo em conexão segura → versão → validação → grava o
  arquivo (temporário + rename) → o host monta um orquestrador novo. **Se o `bootstrap` falhar, o arquivo antigo
  volta** e o hub segue como estava. Com sucesso, o novo orquestrador substitui o antigo.
- **Orquestrador trocável** (`SharedOrchestrator`, um `RwLock<Arc<Orchestrator>>`): cada turno pega o atual uma
  vez e fica com ele até o fim, então um turno em andamento não é afetado. As `RemoteTool`s de um device que
  anunciou tools (Fase 7.4) ficam no `ConnectionOrchestrator`, que remonta a cópia da conexão só quando o
  compartilhado muda (`Arc::ptr_eq`). `Server::bind` aceita `impl Into<SharedOrchestrator>`, então quem passava
  `Arc<Orchestrator>` não mudou.
- **`SettingsHost`** (opt-in via `Server::with_settings`) diz onde está o config e como remontar o orquestrador do
  jeito que aquele processo subiu. O `warden-server` repete os mesmos `--config`/`--provider`/`--model`/
  `--vault-path` e avisa na tela quando uma flag vence o arquivo. O hub embutido do desktop usa o `bootstrap` do
  desktop e, no `installed`, entrega o orquestrador novo também ao chat do desktop (`AppState.orchestrator` virou
  `Arc<Mutex<…>>` para isso).
- **Bug antigo corrigido junto**: salvar as Settings do desktop (ou conectar/desconectar OAuth de MCP) recriava o
  orquestrador do chat, mas o hub embutido continuava com o que tinha subido até ser reiniciado. Agora
  `reload_orchestrator` troca os dois.
- **UI** (em português, como o resto da web): um rascunho só, com "Salvar" e "Descartar" num rodapé fixo. O
  "Salvar" abre o campo da chave de pareamento, que não fica guardada. Um conflito oferece "Recarregar". O
  "customizar" limites parte dos `defaultLimits` que o hub manda, então os números continuam num lugar só.

## Salvar o `config.toml` sem apagar o que foi escrito à mão, e chave de pareamento mínima (P82/P83, Sessão 102)

- **Um ponto de escrita, com merge**: todo save (desktop, web ⚙, CLI, `manage_agents`) continua passando por
  `save_config`, que agora chama `render_config(existing, config)`. O `FileConfig` segue sendo a fonte da verdade
  (serializado com `toml::to_string_pretty` como antes); o `toml_edit` só decide **como** isso cai no arquivo que
  já existe. Nada de editar chave por chave em cada tela: as telas continuam montando um `FileConfig` inteiro.
- **Regras do merge**: valor igual (comparado pelo valor, não pelo texto) fica intacto; valor mudado herda o
  espaço e o comentário de fim de linha do antigo; chave ausente sai com os comentários de cima dela; chave nova
  entra, e tabela nova vai para o fim do arquivo. Array de tabelas casa entradas por `id` ou `name` (posição só
  quando não há nenhum dos dois), e uma entrada nova fica logo depois da anterior. Vazios (`agents = []`,
  `[api_keys]` sem nada) não são acrescentados, porque todo campo desses é `#[serde(default)]`. Arquivo que não é
  TOML válido recebe a saída antiga, inteira.
- **Chave de pareamento com pelo menos 32 caracteres** (`MIN_AUTH_KEY_LEN`, `is_strong_auth_key`): desde a Sessão
  101 ela também protege o salvar configurações, onde um erro custa só 1 s. Escolha do usuário: recusar ao subir,
  não só avisar. Vale para o `warden-server serve` (com `warden-server gen-key` para gerar) e para o hub embutido
  do desktop (salvar o campo e ligar). Devices já pareados não são afetados, porque usam o token deles.

## Tudo pela interface e tudo pelo terminal (Sessão 103)

- **Princípio, pedido do usuário**: o desktop pode ou não ser o hub, e o que o `warden-server` faz pelo terminal
  (para um servidor sem tela) também tem que dar para fazer por uma interface. Os dois caminhos chamam o mesmo
  código (`PairingStore`, `HubTls`, `generate_auth_key`, `is_strong_auth_key`), e cada tela só traduz um formulário
  para ele.
- **Hub do desktop = `serve` inteiro**: cada flag do `serve` tem um campo no `EmbeddedServerConfig` (`listen_host`,
  `tailscale_cert` ou `tls_cert`/`tls_key`/`tls_host`, `web_ui`), com as mesmas regras de validação, checadas ao
  salvar e de novo ao ligar (um `config.toml` editado à mão falha com a mesma mensagem). Campos novos têm default,
  então configs antigos continuam sendo lidos.
- **Um hub sem tela se gerencia pela web**: a aba "Aparelhos" lista, aprova e revoga, como o `warden-server
  devices`. Listar é livre para quem está logado; aprovar e revogar pedem a chave de pareamento a cada vez, com a
  mesma espera de 1 s e o mesmo lock do salvar configurações, então as duas telas dividem o mesmo ritmo de
  tentativas. **A troca da chave de pareamento fica fora da web** de propósito: quem descobrisse a chave poderia
  trocá-la e deixar o dono sem acesso. Ela fica no terminal (`gen-key` + reiniciar) e no desktop da máquina do hub.

## Agentes nomeados no hub e modo "funcionários" (P46, Sessão 104)

- **Um lugar só para "falar como o agente X"**: `warden_bootstrap::agent_scope::scope_to_agent(base, config,
  config_path, agent_id, AgentExtras)` faz o que o desktop (`send_message`) e o CLI (`resolve_turn_context`) faziam
  cada um à sua maneira, e agora o hub também: `with_agent` (skills), os alvos do `delegate_to_agent` montados do
  orquestrador **ainda sem estreitar** (cada alvo fica com a lista própria), `with_allowed_tools` do agente, e as
  tools opt-in (`delegate_to_agent`, `manage_agents`, `message_agent`) por cima. Não troca o modelo nem põe approver:
  o canal decide (no CLI um `/models use` vence o `provider_id` do agente) e cada canal tem o approver que tem.
  `base` precisa vir com o contexto de gasto (P4) já posto, porque delegados e destinatários de recado são clonados
  dele; o desktop passou a pôr o `with_spend_context` antes de escopar.
- **Hub**: `ClientMessage::Chat.agent_id` (opcional, `serde(default)`, o mobile de hoje não muda) e
  `ConversationSummary.agent_id`. O hub relê o `config.toml` pelo `SettingsHost::config_path()` a cada turno com
  agente (sem `SettingsHost`, ou com id inexistente, `ChatError` antes do modelo) e roda `handle_agent_turn`, que é
  o `handle_turn` com persona e grava `conversation.agent_id` a cada turno (`None` limpa: quem manda sem agente não
  tem agente selecionado). O seletor da web lê a lista de agentes do `requestSettings` que ela já usa, sem mensagem
  nova.
- **Aprovação pela conexão**: `warden-server/src/approval.rs::WsApprover`, um por conexão, manda
  `ServerMessage::ApprovalRequest { approvalId, target, action, detail }` ao aparelho que está conversando e espera
  `ClientMessage::ResolveApproval`. Prazo de 120 s, igual ao das tools: ao esgotar, manda `ApprovalCancelled` e conta
  como não. Conexão caída conta como não na hora (`close()` limpa os pendentes), e id desconhecido é ignorado. Vale
  para o `manage_agents`, para host SSH com aprovação e para a pausa de limite de gasto (P4), que usam o mesmo
  approver. Decisão do usuário: aprova quem está no navegador que mandou a mensagem, sem repetir a chave de
  pareamento (como no desktop).
- **Modo "funcionários" = recado para a conversa do colega** (`warden-bootstrap/src/message_agent.rs`), decisão do
  usuário entre três opções (recado, passar a conversa, quadro no vault). `message_agent { action: send|read,
  agent_id, message?, wait? }`:
  - O recado vai para a conversa `thread_id(A, B)` (`agents-` + FNV-1a de 64 bits dos dois nomes; nome de agente é
    texto livre, então o id é um hash estável e seguro para arquivo, e os nomes ficam no título `"A → B"`), no
    diretório de conversas do canal, com `agent_id = B`. Quem abre a conversa e escreve fala direto com B, com os
    recados como contexto.
  - O recado é gravado **antes** de B começar (aparece na lista enquanto B trabalha). B roda numa task destacada, com
    `handle_turn` e a persona, skills, `allowed_tools` e provider **dele** (`delegate_targets`, o mesmo do
    `delegate_to_agent`), a partir do orquestrador base. A resposta, ou o erro, é gravada na mesma conversa.
  - **Parada estrutural contra ping-pong**: B nunca recebe `message_agent`/`delegate_to_agent`/`manage_agents` nem
    approver, porque essas coisas são anexadas por turno e o turno de B não recebe nenhuma. **Um recado em andamento
    por par** (`IN_FLIGHT`, conjunto global por caminho do arquivo, liberado por `Drop`): no máximo um turno de fundo
    por par de agentes, faça o modelo o que fizer.
  - `wait: true` espera até 180 s; depois disso devolve `still_answering` e B continua. `read` diz se B ainda está
    respondendo e devolve as respostas depois do último recado.
  - Opt-in `AgentConfig.can_message_agents`, só ligado por uma pessoa: o `manage_agents` cria sem ele, e ele entrou
    nos `FLAG_GATED_TOOLS` (não pode ser pedido via `allowed_tools`). A spec relê o config a cada iteração, então um
    agente criado no meio do turno já aparece.
  - Aviso de mudança (`ConversationsChanged`): no desktop é o evento Tauri `conversations-changed` (o `App.tsx` pega
    do disco só aquela conversa). No hub é `ServerMessage::ConversationsChanged`, mandado por um sender **fraco**
    (`downgrade`), para um agente que ainda responde depois de o aparelho sair não segurar a task de escrita da
    conexão.
  - **Onde vale**: desktop e hub/web e, desde a Sessão 106, o CLI (os recados vão para a pasta de conversas do
    desktop). No hub, a conversa vai para o diretório do aparelho que mandou a mensagem.
- **Limitações aceitas**: o turno de B não é cobrado do `TurnBudget` do turno de A (é um turno próprio, com o
  próprio teto de delegação); os limites de gasto por período (P4) valem, porque o contexto de gasto vem junto. No
  desktop, ~~o frontend grava conversas sem o lock do bootstrap~~ (resolvido na Sessão 106: o frontend anexa pelo
  `append_messages`).
  O approver da web vale para qualquer aparelho que já pode conversar (inclusive `Pending`), igual ao chat.

## Auto-sync no hub e na web (P61 fatia 1, Sessão 105)

- **Decisão**: o agente trabalha sempre no disco local; o que o P61 chamava de "storage" é para onde esse disco
  sincroniza. Ver a linha "Onde a memória mora vs sync" no registro de decisões.
- **`warden_bootstrap::auto_sync::SyncRunner`**: o corpo do antigo `spawn_auto_sync` do desktop, agora num lugar
  só. Uma rodada relê o `config.toml`: com `[git_sync]` faz pull e depois push (`GitSyncEngine`); sem ele, só pull
  do Arweave (`SyncEngine`), e só se o manifest estiver pareado com o TruthID (sem dono não há de onde puxar, e isso
  não é erro a repetir a cada 5 min). Sem `sync_secrets.json`, nada. Um `tokio::Mutex` serializa rodada, `init` e
  pareamento; o último relatório (`SyncReport`) fica em memória para a tela. `run_loop(interval, on_report)` é um
  future que o chamador roda no próprio runtime (Tauri no desktop, tokio no hub); o callback devolve um future
  (o hub e o desktop recarregam o orquestrador ali). `pair_join(code, host)` usa `join_with_hosts` quando vem um
  IPv4, porque um hub num VPS não está na /24 do aparelho que mostra o código (Tailscale resolve).
- **Hub standalone** (`warden-server serve`): cria o runner com o mesmo config/vault do `bootstrap()` e chama
  `Server::with_sync(runner, Some(AUTO_SYNC_INTERVAL))`. O loop roda numa task abortada quando o `serve_until`
  termina (`AbortOnDrop`). Uma rodada que traz `config.toml` novo recarrega o orquestrador pelo mesmo caminho do
  salvar configurações (`SettingsHost::build` + `installed` + `SharedOrchestrator::replace`); um config com que o
  hub não sobe deixa o orquestrador atual. Sem tela: `warden-server sync status|now|init|pair <código> [--host IP]`
  (o `now` do CLI é outro processo; desde a Sessão 106 ele espera a rodada do `serve` pela trava de arquivo, ver
  "Uma sincronização por vez na máquina").
- **Hub do desktop**: recebe o `Arc<SyncRunner>` do próprio desktop com `loop_every = None` — quem faz o loop é o
  desktop (`sync_cmds::spawn_auto_sync`, que emite os mesmos eventos `auto-sync-pulled`/`auto-sync-pushed` de antes
  e agora também recarrega o orquestrador quando chega config novo). Um lock só para o loop e para o "Sincronizar
  agora" da web.
- **Protocolo**: `RequestSyncStatus` → `SyncStatus { status: SyncStatusDto }` (aberto a qualquer aparelho
  pareado); `SyncAction { pairingKey, action: syncNow | init | pairJoin{code, host?} }` → `SyncStatus` ou
  `SyncError { authRejected }`. A chave é conferida sob o lock das configurações, com a espera de 1 s, mas o lock é
  solto antes da rodada (um push ou pareamento demora e não pode travar um save). `warden-server/src/sync.rs`.
- **Remoto git nas configurações da web**: `HubSettingsDto.gitSync { remoteUrl, token: SecretStatusDto }` e
  `HubSettingsUpdate.gitSync?: { remoteUrl, token: SecretEdit }` (ausente = intocado). O token é segredo como as
  chaves de API (só por conexão segura, nunca volta). Pela web **só `https://`**: um caminho ou `file://` deixaria
  um aparelho pareado apontar os pushes do hub para qualquer diretório da máquina. Remoto não-https continua possível
  à mão no arquivo; a web só manda a seção quando ela foi mexida, para esse caso não travar os outros saves.
- **Web**: aba "Sync" (`SyncView.tsx`) com destino, pendências, última rodada e "Sincronizar agora"; num hub sem
  chave, "Parear com outro aparelho" (código + IP opcional) ou "Este é o primeiro aparelho". Arweave pela web fica
  só no pull automático: o push precisa do QR no celular.
- **Limitações**: ~~o hub só entra num grupo de sync (join); não mostra código para outro aparelho parear com
  ele~~ (resolvido na Sessão 106, ver "O hub mostra um código de pareamento"). ~~Os comandos manuais de git do desktop
  (`git_sync_cmds.rs`) não pegam o lock do runner~~ (resolvido na Sessão 106 pela trava de arquivo nos motores).

### Fatia 2: a camada de Storage Provider saiu (Sessão 105)

- Com a memória sempre local, o seletor de 4 cartões do desktop e a migração entre providers não tinham mais o que
  fazer, e o resto só existia para eles. Removidos, por decisão do usuário ("não gosto de código morto"):
  `warden_core::storage` inteiro (`StorageProvider`, `AuthProvider`, `NoAuthProvider`, `LocalFSProvider`, `migrate`,
  `migrate_interactive`), `DecentralizedVaultProvider`/`TruthIdAuthProvider` do `warden-sync`, `RemoteNodeProvider` e
  `DeviceTokenStore` do `warden-server-protocol`, `vault_node` e o binário `warden-node` do `warden-server`,
  `StorageProviderKind`/`RemoteNodeConfig`/`resolve_storage_provider`/`build_storage_provider`/`build_auth_provider`
  do `warden-bootstrap`, e a seção "Storage" e o modal do QR de migração do desktop. `Vault::delete` ficou (o
  `apply_bundle` do sync usa).
- **Ficou** o roteamento `CallDeviceTool` do hub (Fase 9.3/9.4), decisão do usuário: é genérico, e é nele que o
  "Aprovado" da aba Aparelhos vale. Hoje nenhum cliente o usa.
- **Config antigo continua abrindo**: `FileConfig` é `deny_unknown_fields` e o desktop gravava
  `storage_provider = "..."` em todo save. As chaves `storage_provider` e `[remote_node]` viraram campos legados
  (`legacy_storage_provider`/`legacy_remote_node`, `toml::Value`, `serde(default, skip_serializing)`): lidos e
  ignorados, nunca escritos; como o merge do `render_config` tira chaves que o struct não serializa, o próximo save
  limpa o arquivo (um comentário colado na chave removida sai junto). `WARDEN_STORAGE_PROVIDER` deixou de ser lido.

## Fallback entre provedores (P79) e combos (P90), Sessão 105

- **Config**: `[[combos]]` (`ComboConfig { id, providers }`) no `config.toml`, no mesmo espaço de nomes dos
  provedores: onde se escolhe um provedor (`active_provider`, `AgentConfig.provider_id`, o modelo da conversa no
  desktop, `/models use` no CLI, o gerador de skills) também se escolhe um combo. `check_combos` recusa nome vazio,
  repetido ou igual ao de um provedor, combo sem provedor, provedor desconhecido ou repetido;
  `check_active_provider`/`check_agents` aceitam os dois. Cascatas: renomear provedor atualiza os combos; remover
  provedor o tira dos combos, e um combo esvaziado sai junto com o ativo/agentes que o citavam; `rename_combo`/
  `remove_combo` fazem o mesmo com o ativo e os agentes. As duas telas repetem essas cascatas no rascunho.
- **Primeira versão (substituída na mesma sessão)**: uma lista global `fallback_providers`, aplicada depois do
  provedor de qualquer turno. Saiu por decisão do usuário, porque um combo é essa lista com nome. Um config antigo
  a lê como campo legado (`legacy_fallback_providers`, só leitura) e, na carga (`migrate_legacy_fallbacks`), ela
  vira o combo `"<ativo>-reserva"` = [ativo, ...lista], que passa a ser o ativo; o próximo save grava o combo.
- **Resolução**: `build_model_for(config, id, model_override)` — provedor → `build_model_provider`; combo →
  `FallbackProvider` sobre os provedores dele (`combo_chain` pula, com um aviso, o que não constrói; sobrando um,
  devolve ele puro; nenhum, erro). `model_override` (a flag `--model`) só vale para provedor.
- **`warden_core::model::FallbackProvider`**: o provedor do turno primeiro, depois os reservas. Só troca **antes** de
  o stream começar e só num erro que diz "tente em outro lugar": `ProviderHttpError` (novo, tipado, com o mesmo
  `Display` do antigo `bail!` dos três providers) com 408/429/5xx, ou um `reqwest::Error` de conexão/timeout. Um
  400/401/403 ou qualquer outro erro do provedor do turno volta na hora — trocar esconderia um problema real. Já em
  fallback, um reserva que falha por qualquer motivo passa para o próximo; todos falhando devolve o erro do
  primeiro com os outros como contexto. O limite de gasto do Warden (P4) barra antes da chamada, então nunca
  dispara troca.
- **Quem respondeu**: `StreamEvent::ProviderFallback { from, to, model, reason }` sai primeiro no stream do reserva.
  O orquestrador o intercepta ao drenar o stream: grava o gasto no `model` do reserva (não no `model_id()` do
  turno) e junta as trocas em `MessageOutcome.fallbacks`, uma por par de provedores mesmo com várias voltas de
  tools. O `Response` não mudou (evitou mexer nas dezenas de literais dele nos testes).
- **Onde vale**: `build_model_for` em todo lugar que parte do config: o modelo ativo (`resolve_model_provider`, só o
  caminho do registro), agentes com modelo próprio no hub, no desktop e nos alvos de delegação, o seletor de modelo
  do CLI e o gerador de skills.
- **Aviso**: `ChatResponse.fallbacks: Vec<ProviderFallbackDto>` (omitido quando vazio; mobile e extensão ignoram) →
  linha "Respondido por X — Y falhou (motivo)" acima da resposta na web; o `send_message` do desktop devolve o
  mesmo e o `MessageBubble` mostra (não é gravado na conversa); o CLI imprime uma linha esmaecida. Telegram e
  WhatsApp: só o log.

## O hub mostra um código de pareamento (P88 item 1, Sessão 106)

- **Problema**: o hub só entrava num grupo de sync. Um aparelho novo não tinha como receber a chave de um hub sem
  tela, e quem digita o código (desktop, `/sync pair` do CLI) só varria a rede local, então um hub num VPS nunca
  seria achado.
- **`SyncRunner::start_hosting`**: abre um `PairingHost` (o mesmo do desktop), dá `spawn` no `wait_for_join` e
  guarda a sessão (código, validade, `AbortHandle`) num `Arc<Mutex<Hosting>>`. Pedir de novo com uma sessão aberta
  devolve o mesmo código, sem abrir outra porta. Ao terminar, a task limpa a sessão e grava `last_pairing` (ok ou
  o erro); um número de sessão impede que uma task velha apague uma sessão mais nova. `cancel_hosting` aborta a
  task, e o `TcpListener` cai junto.
- **Fora do lock das rodadas**: hospedar só lê a chave e não mexe no disco. Segurar o lock por até 5 minutos
  travaria o loop e o "Sincronizar agora".
- **O código só vai na resposta ao `PairHost`**: `SyncStatus.pairingCode` (fora do `SyncStatusDto`). O status que
  qualquer aparelho pareado lê só traz `hostingUntilMs` e `lastPairing`, porque quem tem o código leva a chave do
  vault. A web guarda o código e a chave digitada só no estado da tela, para o "Parar de mostrar" não pedir a chave
  de novo, e recarrega o status a cada 3 s enquanto o código está aberto.
- **Protocolo**: `SyncActionDto::PairHost` e `CancelPairHost`, os dois com a chave do hub como toda ação.
- **Sem tela**: `warden-server sync host` mostra o código, as portas (48070 a 48074) e espera. Pode rodar com o
  `serve` no ar, porque não mexe no disco; se a web já estiver hospedando, ele pega a próxima porta livre da faixa.
- **Quem digita o código aceita um IP**: `pairing_join` do desktop (`host?`, campo "IP (opcional)" na tela Sync) e
  `/sync pair <código> [ip]` no CLI usam `pairing_join_with_hosts`, como já faziam a web e o `sync pair --host`.

## Uma sincronização por vez na máquina, entre processos (P88 itens 2 e 3, Sessão 106)

- **Problema**: o mutex do `SyncRunner` só vale dentro de um processo. Os botões de git do desktop, o
  `warden-server sync now` com o `serve` no ar e o `/sync` do CLI montam motores próprios sobre o mesmo vault, o
  mesmo `sync_manifest.json` e o mesmo clone git, e podiam cruzar com uma rodada automática.
- **Decisão**: a trava fica **nos motores**, não em cada chamador. `warden_sync::lock::SyncLock` é uma trava de
  arquivo do SO (`std::fs::File::try_lock`, sem crate novo, igual em Linux/macOS/Windows) em
  `sync_manifest.lock`, ao lado do manifest que os dois motores dividem. Ela tenta a cada 250 ms, com espera
  assíncrona, desiste em 10 minutos e sai quando o arquivo fecha. Assim qualquer processo, incluindo o mobile, se
  serializa sem saber dos outros.
- **Quem pega a trava**: `GitSyncEngine::push`/`pull` e `SyncEngine::pull`/`finish_push`/
  `finish_push_with_hosts`. O `begin_push` não pega, porque só lê e depois espera o QR no celular. O `finish_push`
  segura a trava enquanto espera o celular fixar o bundle, o que pode atrasar uma rodada automática por alguns
  minutos, mas nunca cruzar com ela. `init_fresh`, a adoção da chave no pareamento e o `status()` ficam fora.
- **Nunca aninhar**: no Linux, uma segunda trava por outro descritor do mesmo arquivo espera a primeira, mesmo no
  mesmo processo. Nenhum método que trava chama outro que trava. O `SyncRunner` mantém o mutex em memória por
  cima, que continua cobrindo `init`/`pair` e o último relatório.

## Agentes e aprovação no mobile e na extensão, recados pelo CLI, fim da corrida no desktop (P87, Sessão 106)

- **Escrita de conversa entre processos**: `append_to_conversation`/`rename_conversation`/`delete_conversation`
  passam por `ConversationWriteGuard`, que junta o mutex do processo (`CONVERSATION_WRITES`) com uma trava de
  arquivo bloqueante (`File::lock`) em `<pasta>/.writes.lock`, só em volta da leitura e escrita do arquivo. A
  listagem só lê `.json`, então a trava nunca aparece como conversa. Pública agora:
  `append_messages(dir, id, AppendOptions { title_seed, agent_id, provider_id, create }, messages)`, que devolve a
  conversa como ficou no disco (`provider_id: None` deixa como está, porque só o desktop guarda um).
- **Desktop**: o frontend deixou de gravar a conversa inteira (`save_conversation` saiu). Ele mostra a mensagem na
  hora e chama `append_conversation_messages`; a cópia do disco volta e entra no estado por `replaceWithSaved`, que
  mantém no fim o que ainda não chegou ao disco e o aviso de fallback (P79), que só é mostrado e nunca salvo. O
  evento `conversations-changed` usa a mesma junção. Assim uma resposta de B, ou um recado do CLI, não é apagada.
- **CLI**: `resolve_turn_context` passa `AgentExtras { conversations_dir: default_conversations_dir() }`, então o
  agente com `can_message_agents` recebe `message_agent`. A conversa "A → B" aparece no desktop na próxima leitura da
  lista (não há evento entre processos), e o `read` funciona no terminal. Como a resposta de B roda no processo do
  CLI, sair (`/exit`, `exit`, Ctrl+D) espera até 180 s por `message_agent::answers_in_flight()`, e Ctrl+C sai já.
- **Mobile e extensão**, mesmo desenho da web:
  - os ids dos agentes vêm do `requestSettings` (só `settings.agents[].id` é lido);
  - o agente vai no `chat` como `agentId`; abrir uma conversa restaura o `agentId` dela e uma nova mantém a última
    escolha;
  - o `ApprovalRequest` vira um diálogo (mobile) ou um cartão no topo do painel (extensão), em fila, fechado pelo
    `ApprovalCancelled`;
  - o `ConversationsChanged` recarrega a lista e, se a conversa está aberta e sem turno pendente, o histórico.
- **Avisos quando a tela não está à vista**: no mobile, uma notificação local "Approval needed" com id próprio (não
  substitui a da resposta). Na extensão, um selo "!" no ícone enquanto houver aprovação pendente, porque o painel
  pode estar fechado.
- **Tipos desconhecidos**: o mobile passou a devolver `UnknownServerMessage` em vez de lançar exceção, e a extensão
  ignora com um aviso no console, para um hub mais novo não quebrar a conexão.

## Warden API (P12, Sessão 106)

- **O que é**: o agente do hub no formato de chat completions da OpenAI, no mesmo porto do WebSocket e da
  web. Qualquer cliente OpenAI (script, n8n, app de chat) aponta a base URL para `http(s)://<hub>/v1`, usa
  uma chave criada no app e fala com o Warden: vault, skills, tools e, com `model: "warden/<agente>"`, a
  persona e o escopo daquele agente (`scope_to_agent`).
- **Roteamento** (`server.rs::serve_web_or_ws`): a cabeça da requisição é sempre lida, com ou sem web UI.
  Um upgrade de WebSocket segue para o protocolo, `/v1/` vai para `openai_api.rs`, e o resto vai para a
  página ou recebe `404`. Num hub só-TLS, `http://` recebe o mesmo redirecionamento (ou `426`) da página,
  inclusive na API.
- **HTTP à mão**, como o `web_ui.rs`: `RequestHead` passou a trazer os cabeçalhos e onde o corpo começa. O
  corpo vem pelo `Content-Length` (até 8 MiB; chunked → `411`), uma requisição por conexão, com
  `Connection: close`. O SDK oficial em Python aceita.
- **Chaves** (`api_keys.rs`, `~/.config/warden/api_keys.json`):
  - `wdn_` + 64 hex; só o SHA-256 vai ao disco, com `shown` (os 12 primeiros caracteres) para a lista;
  - o arquivo é relido a cada chamada, então revogar vale na próxima requisição, até de outro processo;
  - o último uso é regravado no máximo uma vez por minuto.
  Chave errada ou ausente → `401 invalid_api_key` depois de 1 s (a mesma taxa da chave de pareamento).
- **Turno**:
  - `system`/`developer` do cliente é somado à persona; `user`/`assistant` viram o histórico, com os
    `tool_calls` e as mensagens `tool`; a última mensagem é do usuário ou um resultado de tool.
  - Uma parte que não é texto (imagem) → `400`.
- **Tools do cliente** (P91, Sessão 107):
  - As `tools` do cliente (só `type: "function"`) vão para `Orchestrator::with_client_tools` e são
    oferecidas ao lado das do agente. Uma tool do agente com o nome de uma do cliente some do pedido.
    Só a raiz do turno as oferece: um subagente não tem como devolver a chamada.
  - Quando o modelo chama uma delas, o turno para e `MessageOutcome.client_tool_calls` volta. A resposta
    sai com `tool_calls` e `finish_reason: "tool_calls"`. As tools do agente pedidas na mesma resposta
    não rodam, porque a chamada delas não teria como ir ao cliente, e o modelo pode pedi-las de novo.
  - O cliente roda a tool e reenvia a conversa com as mensagens `tool` no fim. O hub chama
    `Orchestrator::resume_turn_streaming`, que continua sem mensagem nova do usuário e busca no vault
    pela última mensagem do usuário. Cada continuação conta como um turno novo para os limites.
  - Os ids entregues são do hub (`call_<24 hex>`), porque o Gemini repete `call_0` em toda resposta.
    A `thought_signature` do Gemini, que o Gemini 3 exige de volta, vai dentro do id
    (`call_<hex>__ts_<base64url>`). Na volta, o provedor recebe o id curto (a OpenAI limita a 40
    caracteres) e a assinatura no campo dela. Um id que o hub não fez passa inteiro.
  - `tool_choice: "none"` tira as tools do cliente. Qualquer outro valor conta como `auto`, porque o
    `ModelProvider` não tem escolha de tool. `parallel_tool_calls` é ignorado.
  - Com `stream`, o texto sai ao vivo e as chamadas vão num chunk cada, no fim, antes do `finish_reason`.
    Só no fim se sabe se uma chamada era do cliente.
  - Um `tool_call_id` que não responde a nenhuma chamada anterior → `400`.
  - O orquestrador compartilhado roda com `SpendContext::new("api").with_user(<nome da chave>)`, então os
    limites (P4) valem, e um limite atingido volta como `429` com `code: "spend_limit:<id>"`.
  - Sem approver, o que pede aprovação é recusado; sem pasta de conversas, não há `message_agent`.
  - Com `stream`, o SSE traz `chat.completion.chunk` a cada `ContentDelta` do `handle_turn_streaming`, um
    `finish_reason: "stop"`, o `usage` quando o cliente pede `stream_options.include_usage`, e `[DONE]`.
- **Gestão**:
  - protocolo `ListApiKeys` (aberto a aparelhos pareados), `CreateApiKey`/`RevokeApiKey` (com a chave de
    pareamento, o lock e a espera das configurações) → `ApiKeyList`/`ApiKeyCreated`/`ApiKeyError`;
  - na web, a seção "Warden API" das Configurações; no desktop, a seção em Settings (comandos Tauri sobre o
    mesmo arquivo), fora do formulário;
  - no terminal, `warden-server api-keys list|create|revoke`.
- **Chave presa a um agente** (pedido do usuário, mesma sessão): `ApiKey.agent_id` (ausente = geral, e chaves
  antigas continuam gerais).
  - Uma chave presa a `X` só fala como `X`: `model` ausente, `warden` ou `warden/X` viram `warden/X`, e outro
    agente → `403 model_not_allowed`.
  - O `/v1/models` dela lista só `warden/X`. Se `X` sumiu do config → `403 agent_gone`, nunca o padrão do hub.
  - Quem cria confere que o agente existe (`api_key_admin::check_agent_exists`): o hub pelo `SettingsHost`, o
    desktop e o CLI pelo config. A web e o desktop têm um seletor "Geral / Só o agente X", e o CLI tem
    `--agent`.
  - O gasto já saía por agente (o `SpendTurn` usa o `agent_id` do orquestrador), então um limite de escopo
    `agent` vale para essas chamadas.
- **Fica de fora**: imagens, `/v1/embeddings` e outros endpoints, e keep-alive. O repasse das tools do
  cliente entrou na Sessão 107 (P91, acima).

## Rede de nós no mesmo workspace (P86, estudo da Sessão 107 — nada implementado)

Registro de uma conversa de desenho com o usuário, para quando for construir. A ordem combinada está no
`ROADMAP.md` ("Rede de nós no mesmo workspace"): P92 e P93 vêm antes.

### O modelo

- **Workspace** é *o* Warden: agentes, vault, skills, provedores, conversas, aparelhos, chaves da API e limites.
  É uma coisa só, em quantas máquinas estiver.
- **Nó** é uma máquina que entrou no workspace (VPS, desktop, mini-PC, outro VPS). Nenhum é "o servidor", e
  qualquer um pode:
  - **atender**: celular, web, extensão e a Warden API falam com o nó que estiver de pé;
  - **executar**: tarefas agendadas (P92), jobs e agentes trabalhando sozinhos. Se o nó que ia rodar caiu, outro
    assume;
  - **oferecer o que só ele tem** (P93): o shell e os arquivos da máquina, uma GPU, um modelo local, uma rede
    interna.
- Primeira leitura errada, corrigida pelo usuário: não é *failover* (um servidor de reserva para a API). A API se
  manter no ar é só uma consequência.
- Não confundir com o `warden-node` removido na Sessão 105: aquele era só um nó de armazenamento do vault.

### Onde fica o estado: sem centro

Opções pesadas: (1) sem centro, cada nó com a sua cópia; (2) um nó âncora com a verdade; (3) um serviço externo
(banco, Warden Cloud do P50). **Preferência do usuário: sem centro**, qualquer nó pode cair, inclusive o principal,
"mas precisamos trabalhar bem na parte de conflitos".

- **Como os nós trocam o estado**:
  - cada nó guarda um **registro de operações** ("mensagem X adicionada", "aparelho Y revogado", "campo Z do
    agente mudou"), não só o estado final, cada operação com um relógio **HLC**;
  - quando dois nós se encontram, comparam o que cada um já viu e mandam só o que falta, pelo WebSocket que já
    existe;
  - um celular pode carregar mudanças de um nó para outro que nunca estão no ar ao mesmo tempo;
  - o git e o Arweave de hoje continuam como mais um caminho para o registro (backup, ou nós que nunca se veem).
- **CRDT**: estruturas cuja junção dá o mesmo resultado em qualquer ordem, porque nós separados (o notebook no
  avião e o VPS) continuam funcionando e precisam se juntar sozinhos depois. Bibliotecas Rust a avaliar num
  protótipo antes de escolher: **Automerge**, **Loro** e **yrs** (Yjs). Critérios: tamanho do histórico,
  desempenho com um vault grande e compactação do registro antigo.
- **Limite de base**: "rodar uma tarefa exatamente uma vez" é impossível sem centro. Cada lado de uma separação
  pode achar que o outro caiu. É preciso escolher, por tarefa, qual erro é aceitável.

### Conflitos, tipo por tipo

| Dado | Regra de junção | Conflito que sobra |
|---|---|---|
| Conversas | Cada mensagem com id e HLC; junção = união dos conjuntos, ordenada. A escrita por anexo (P87, `append_messages`) já combina | Dois nós respondendo à mesma conversa ao mesmo tempo: as duas respostas ficam, em ordem |
| Aparelhos e chaves da API | Conjunto com lápide; **revogar sempre vence** | Nenhum: um aparelho revogado nunca volta por uma cópia velha |
| Gasto e limites (P4) | Contador por nó, total = soma | Numa separação, cada lado só vê o seu gasto e pode passar um pouco do limite. Mitigação: dividir o limite entre os nós enquanto separados |
| Agentes, provedores, combos, skills | Mapa por id, e vence a mudança mais recente de cada campo | Dois nós no mesmo campo do mesmo agente: vence um, e o outro fica no histórico para ver e recuperar |
| Vault (markdown) | O caso mais difícil. Hoje o sync é por arquivo inteiro e só se recusa a sobrescrever mudança não enviada, sem juntar. O certo é um CRDT de texto | Edições no mesmo trecho ficam lado a lado. Alternativa: juntar por parágrafo e marcar o conflito no arquivo, como o git |
| Tarefas agendadas | Definição = mapa, como os agentes. Execução: dono calculado igual por todos a partir dos nós vivos, com reserva e prazo; se o dono cair, o próximo assume quando o prazo vence | Numa separação, a tarefa pode rodar duas vezes. Cada tarefa declara "pode repetir" (resumo diário) ou "no máximo uma vez" (na dúvida, não roda) |
| `config.toml` com comentários (P82) | O arquivo escrito à mão não é um CRDT: a verdade passa a ser os dados estruturados, e o `.toml` é gerado e editável, com a edição virando operações no mapa | — |
| OAuth de MCP, certificado TLS | Continuam por máquina, sem réplica | — |

### Por que ficou para depois

Avaliação feita a pedido do usuário ("na sua opinião, isso vale a pena?"), aceita por ele:

- o custo de sistema distribuído é desproporcional para uma pessoa só: meses de trabalho e bugs difíceis de
  reproduzir. Um VPS fica meses no ar, então a disponibilidade comprada é pequena para um usuário;
- o cenário ainda não existe ("ainda não sei, vou usar em vários dispositivos"), e a arquitetura mais cara para
  um uso imaginado costuma errar o alvo;
- a base tem muita coisa sem teste real (P80);
- cruza com o P84 (multiusuário): conflitos entre nós *e* entre pessoas é bem mais difícil que cada um separado.
  Vale decidir antes se o Warden é de uma pessoa ou de várias.

O que o usuário mais quer ("rodar tarefas a qualquer momento", usar vários aparelhos como parte do Warden) sai de
**P92** e **P93**, sem estado descentralizado. Se a queda do nó principal virar problema real, descentraliza-se
primeiro só os dados fáceis (aparelhos e chaves, conversas, gasto), que já bastam para outro nó assumir a API e as
conversas; o vault fica no sync atual. O resto (agentes, configuração, vault com junção de texto) é o P86 completo.

## Multiusuário no mesmo workspace (P84, desenho da Sessão 107; fatias 1 e 2 feitas nas Sessões 110 e 111)

Registro de uma conversa de desenho com o usuário. Hoje o Warden é o agente pessoal de uma pessoa. A ideia é que,
numa família (ou numa empresa, "precisa ser do mesmo jeito"), cada pessoa tenha os seus agentes e o seu vault sem
misturar, com o usuário como **root** configurando as permissões. Nas palavras dele: pegar a ideia, sem ter
pensado em todas as possibilidades.

### O modelo

| Conceito | O que é |
|---|---|
| **Usuário** | Uma pessoa. Papéis: **root** (o dono do workspace), **membro** e talvez **convidado** (só usa o que foi liberado). Aparelhos pareados e chaves da Warden API passam a pertencer a um usuário |
| **Espaço de memória** | Um vault. Cada usuário tem o **seu**, isolado. O root pode criar **espaços compartilhados** ("casa", "viagens") com quem lê e quem escreve em cada um |
| **Agente** | Tem um **dono** (uma pessoa) ou é **do workspace**. Para cada agente se define quem pode usá-lo e com quais limites. Cada pessoa cria os próprios agentes, dentro do que o root permitir |
| **Permissão** | Sempre a mesma forma: **quem** (usuário ou grupo) pode **o quê** (usar, ver, editar, administrar) **em quê** (agente, espaço, conversa, tool, provedor, limite de gasto) |

Exemplos que o modelo cobre:

- "minha mulher usa meu agente, sem shell e com até R$ 20 por mês": permissão de uso no agente, com restrição de
  tools e limite de gasto;
- "o agente da casa lê o espaço 'casa' e nunca o meu vault": o agente com acesso só àquele espaço;
- "a chave da API dela só fala com os agentes dela": a chave herda as permissões do usuário que a criou, e pode ser
  presa a um agente, como já é hoje (P12).

Peças que já existem e seriam aproveitadas: `allowed_tools` por agente, chave da API presa a um agente, limites de
gasto com escopo "usuário" (P4), skills com escopo por agente, aprovação de tools. Falta o conceito de pessoa
amarrando tudo.

### Privacidade e backup

Escolha do usuário: **o root não lê o vault nem as conversas dos outros**. Mas ele tem medo de perder os dados de
alguém numa manutenção ("tá criptografado e eu perco acesso"). O conflito é real: **conseguir recuperar é
conseguir ler**. A saída é separar copiar de ler:

- **Backup sempre**: o root copia tudo, mas o que é de cada pessoa vai **criptografado com a chave dela**. Um backup
  completo e ilegível para o root.
- **Recuperação por política do workspace**, decidida na criação e visível para todos os membros:
  - **privado de verdade**: só a pessoa recupera, com um código de recuperação dela. Se perder a senha e o código,
    perdeu;
  - **recuperável com consentimento**: a chave dividida em partes (Shamir, 2 de 3: a pessoa, o código de
    recuperação dela e o root). O root sozinho não abre; o root mais o código da pessoa abrem. O melhor para
    família;
  - **recuperação de empresa**: o admin recupera sozinho, mas cada uso fica registrado e a pessoa é avisada.
- **Aviso honesto, a mostrar para os membros**: o agente precisa ler o vault para trabalhar, e roda no hub. Quem
  controla a máquina do hub sempre pode, tecnicamente, ver o que o agente vê enquanto ele trabalha. A criptografia
  protege backups, discos e cópias, e impede leitura casual, mas não protege contra quem tem controle total do
  servidor. Para esse nível, o agente da pessoa teria que rodar num aparelho dela (liga com P86/P93).

### Conversas compartilhadas

Privadas por padrão. O dono pode compartilhar uma conversa com outros usuários, para **leitura** ou
**participação** (a outra pessoa também escreve). É o mesmo formato de permissão.

### O agente de alguém falando com outra pessoa

Pedido do usuário: com o agente dele, a memória é a dele, mas ele quer decidir o que o agente pode responder para
a esposa, "ensinando" o agente ou por uma interface.

- **A proteção não pode ser o prompt**: uma instrução ("não conte X para ela") vaza, porque o modelo pode ser
  convencido ou errar.
- **Camada que garante**: as notas têm **audiência**, por pasta ou etiqueta ("só eu", "família", "ela"). Quando o
  agente fala com uma pessoa, ele só **enxerga** as notas liberadas para ela: busca no vault, memória fixa e leitura
  de arquivos filtradas antes de chegar ao modelo.
- **Camada de comportamento**, por cima: o dono ensina o tom e o que evitar ("sobre dinheiro, responda só de forma
  geral"), por texto ou por uma tela. Ajuda, mas não é o que protege.

### Login

- **Todo mundo tem nome de usuário.** Não existe autocadastro: só o root cria usuários.
- **Senha**: o root cria o usuário com uma senha provisória, e a pessoa troca no primeiro acesso.
- **TruthID**: o root cria o usuário e gera um **convite** (código ou QR); a pessoa abre o convite e liga o TruthID
  dela àquele usuário, e daí em diante entra por ele. Um usuário pode ter os dois. O fluxo exato precisa de estudo
  próprio (o `warden-truthid` do P38 nunca foi testado contra o app real).
- **Aparelho pareado entra como alguém**: o token do aparelho fica preso a um usuário, e tudo o que o aparelho faz
  vale com as permissões dele.

### Em aberto

- Onde fica cada vault (pastas por usuário no mesmo hub; e com a rede de nós do P86, como os conflitos entre nós
  se combinam com as permissões entre pessoas).
- Grupos de usuários, e como a interface de permissões fica simples para uma família.
- O que o root vê sem ler: tamanho, gasto, último acesso.
- A saída dos arquivos fixos do vault (P94), porque um perfil fixo por vault não faz sentido com vários usuários.

### As fatias (Sessão 110)

1. **Pessoas e login** (feita na Sessão 110): usuários, root e membros, login por usuário e senha, aparelho preso a
   uma pessoa, vault e conversas por pessoa.
2. **Permissões nos agentes** (feita na Sessão 111): quem usa qual agente, com quais tools e limites de gasto, agentes
   próprios e chaves da Warden API por pessoa.
3. **Espaços compartilhados e audiência das notas**: o agente do root falando com outra pessoa só enxerga o que foi
   liberado para ela.
4. **Criptografia, backup e recuperação**, com as três políticas acima. **Parte A feita na Sessão 114**
   (criptografia, backup e "privado de verdade"); **a parte B também na Sessão 114** (recuperação com
   consentimento e de empresa, sem Shamir literal).
5. **Convite pelo TruthID.** **Feita em parte na Sessão 115**: o convite e o vínculo; o login ficou no P113.

### Como ficou a fatia 1 (Sessão 110)

- **Decisões do usuário**:
  - os usuários ficam no `config.toml` (`[[users]]`), que sincroniza entre as máquinas do root, com as senhas só
    como hash Argon2id;
  - as conversas passam a ser **da pessoa**, as mesmas em todos os aparelhos dela, e as que já existiam viram do
    root;
  - um membro que usa um agente do root leva a persona e as tools do agente, mas **com a memória dele**. Usar a
    memória do root com outra pessoa espera a audiência (fatia 3, feita nas Sessões 112 e 113 como pastas
    compartilhadas).
- **Root**: continua sendo quem tem a chave de pareamento. Não aparece no `[[users]]`, e os aparelhos pareados com
  a chave são dele (`PairedDevice.user = None`). As telas de administração seguem pedindo a chave. Sem
  `[[users]]`, nada muda para quem já usava.
- **Membro** (`warden_bootstrap::users`): `UserConfig { id, name, role = "member", password_hash,
  must_change_password }`. O id tem de 1 a 32 caracteres (letras minúsculas, dígitos, `-` ou `_`), e `root` é
  reservado.
  - O root cria o membro, e a senha provisória (14 caracteres, sem letras parecidas) aparece uma vez.
  - `authenticate_user` confere a senha até de um nome que não existe, contra um hash-isca, para o tempo de
    resposta não revelar quem existe.
- **Pareamento** (`device_registry.rs`): `PairingProof` (`Nothing`, `PairingKey` ou `Member(id)`) e
  `authenticate_as`.
  - Pareia pela senha: o aparelho vira do membro. Pareia pela chave: vira do root. O token mantém o dono.
  - Um aparelho que volta com o token pode mandar também a senha velha, e quem decide é o token.
  - Senha errada espera 1 s, como a chave errada.
  - `revoke_user_devices` revoga os aparelhos de um membro removido. Um aparelho cujo membro sumiu do config é
    recusado no `Hello`, o que cobre o hub do VPS depois do sync.
- **Protocolo**:
  - `Hello.username/password` e `HelloAck.user` (`UserInfoDto`, ausente para o root e para hubs antigos);
  - `ChangePassword`, e `ListUsers`, `SaveUser` (criar com `is_new`, ou renomear), `ResetPassword` e `RemoveUser`
    com a chave, respondidos por `UserList { temp_password }`, `PasswordChanged` ou `UserError`;
  - `DeviceDto.user` mostra de quem é cada aparelho.
- **Na conexão** (`people.rs`), `Person::Root` ou `Person::Member(MemberSpace)`, com vault em
  `~/.config/warden/users/<id>/vault`, arquivos gerados em `…/<id>/generated` e conversas em
  `conversations-server/users/<id>/`. Para um membro:
  - **orquestrador**: `member_orchestrator` aplica, depois de escopar o agente, `with_allowed_tools(allowlist)`,
    `with_vault`, `with_media_root` e o gasto como `server:user-<id>`;
  - **allowlist** (`member_may_use`): arquivos, skills, `delegate_task`, `jobs`, `budget`, `generate_document` e as
    tools `tavily*`. Ficam de fora o shell, o SSH, os nós, as MCP do root, `delegate_to_agent`, `message_agent`,
    `usage_stats`, `manage_agents` e `manage_tasks`. É lista de permitidas de propósito: uma tool nova fica longe
    dos membros até alguém decidir, na fatia 2;
  - **`Orchestrator::with_vault`** (núcleo): troca o vault do contexto e da memória fixa, e religa cada tool presa a
    um vault (`Tool::with_vault`: arquivos, shell, as três de skills e o `delegate_task`, que carrega um
    orquestrador dentro). Um sub-agente do membro também escreve no vault dele;
  - **recusas**: `member_refusal` recusa a administração e as visões do hub inteiro (aparelhos, chaves da API, nós,
    tarefas, sync, uso, pessoas, `CallDeviceTool`, salvar configurações), cada uma com o erro do seu tipo e
    `auth_rejected`. `RequestSettings` responde só com os agentes (`member_settings_view`);
  - **senha provisória**: com ela, `password_gate` deixa passar só `ChangePassword`, `Ping` e `Goodbye`;
  - **vault e skills**: as telas usam o vault do membro;
  - **tarefas**: `ConversationsChanged` das tarefas não chega a membros, e `task-*` não aparece na lista deles.
- **Migração**: na subida do hub, `migrate_device_conversations` move `conversations-server/<aparelho>/` (e o
  `<aparelho>.json` de antes do P78) para `conversations-server/root/`. Um id repetido (o `default` de cada
  aparelho) vira `<id>-<aparelho>`, e um arquivo ilegível é movido como está. A função `device_conversations_dir`
  saiu.
- **Uso**: `RequestUsage` mostra uma linha "Owner" e uma por membro (`user:<id>`).
- **Telas**:
  - web: login com as abas Usuário e Chave de pareamento, troca de senha obrigatória, e a aba Pessoas só para o
    root. Para um membro, somem Uso, Tarefas, Aparelhos, Sync e Configurações;
  - mobile: "Pairing key / Username" na conexão e o diálogo de troca de senha antes do chat;
  - desktop: a seção People no Workspace, pelo `config.toml` local e sem pedir a chave;
  - CLI: `warden-server users list|add|reset-password|remove`.
- **Fora desta fatia**: a extensão e o CLI remoto seguem só com a chave (root). O chat do próprio desktop é do root.
  O membro não cria agentes e não tem Warden API. A memória fixa (`_profile.md` etc.) do vault do membro nascia
  vazia; o P94 (Sessão 115) acabou com ela.

### Como ficou a fatia 2 (Sessão 111)

- **Decisões do usuário**:
  - o membro só usa os agentes que o root compartilhar, e os que já existem começam só do root;
  - as tools são uma lista por pessoa, que começa no conjunto seguro da fatia 1;
  - os membros criam agentes próprios, com dono;
  - cada membro tem as próprias chaves da Warden API.
- **Agentes** (`AgentConfig`), com dois campos novos:
  - `owner` (`None` é do root);
  - `shared_with` (ids de membros ou `"*"`, só nos agentes do root).
- **Visibilidade**: uma função só decide quem vê cada agente, `users::agent_visible_to`. O root vê os dele; o membro
  vê os dele e os compartilhados. Ela vale no chat, na Warden API (`/v1/models` e o `model`), nas chaves presas a
  agente e no seletor.
- **Onde os agentes dos membros ficam de fora**:
  - a tela do root não mostra e, ao salvar, preserva os agentes dos membros (`apply_hub_settings`, o save do desktop
    e o `/agents` do CLI);
  - `delegate_to_agent`, `message_agent`, `manage_agents` e as tarefas agendadas só enxergam os agentes do root.
- **Tools por pessoa** (`UserConfig.tools`):
  - `None` é o padrão (`default_member_tool`, que saiu do `people.rs`);
  - `set_user_tools` descarta `NEVER_FOR_MEMBERS` (`delegate_to_agent`, `message_agent`, `manage_agents`,
    `manage_tasks`, `usage_stats`). Essas tools nunca são de um membro, porque alcançam orquestradores e conversas
    que o `with_vault` não religa;
  - o turno fica com a interseção entre as tools da pessoa (`member_tools`) e as do agente (o `allowed_tools` já
    aplicado ao escopar). O hub relê o config a cada turno, então uma mudança do root vale na hora.
- **Agentes próprios** (`save_member_agent` e `delete_member_agent`, pelas mensagens `SaveOwnAgent` e
  `DeleteOwnAgent`):
  - ficam sempre com o membro como dono, sem compartilhamento e sem `can_*`;
  - as tools são cortadas às da pessoa;
  - nome único no config inteiro;
  - só o dono edita ou apaga;
  - `remove_user` leva os agentes junto e tira a pessoa de todo `shared_with`.
- **O que o membro vê**: `member_settings_view` monta os agentes que ele enxerga:
  - os próprios completos, com `owner`;
  - os compartilhados só pelo nome, porque a persona do root pode ter coisa privada;
  - `tool_names` com as tools que ele tem.
- **Gasto por pessoa**: `Scope::Person(id)` no núcleo.
  - `SpendContext.person` e `SpendEvent.person`; o evento com `serde(default)`, então o ledger antigo continua lido;
  - um `[[limits]]` com `scope = "person"` e `target = "<id>"` cobre a pessoa na web, no celular e na API;
  - o escopo entrou nos editores de limites (web, desktop e o `/limits` do CLI).
- **Sem aprovador para membros**: um turno de membro não recebe o aprovador. Ele não pode aprovar a própria extensão
  de limite (o que anularia o limite do root), e uma tool que espera o "sim" do root é recusada. Achado pelo teste de
  integração.
- **Chaves da API por pessoa** (`ApiKey.user`):
  - o membro lista, cria e revoga só as dele, confirmando com a **própria senha** no campo `pairing_key`, com a
    mesma espera de 1 s no erro;
  - só prende a chave a um agente que ele vê;
  - uma chamada pela chave dele roda em `member_orchestrator` (canal `api`, `person` marcado);
  - remover o membro revoga as chaves dele;
  - a lista do root mostra de quem é cada chave.
- **Telas**:
  - web do membro: as abas **Agentes** (`MyAgentsView`) e **API** (a `ApiKeysSection` no modo membro);
  - web do root: "Compartilhar com" nos agentes, ferramentas por pessoa na aba Pessoas (com aviso nas que alcançam o
    que é do root), escopo "Uma pessoa" nos limites e o dono de cada chave;
  - desktop: "Shared with", as tools na seção People (`set_person_tools`), o escopo "One member of the workspace"
    nos limites e o dono das chaves;
  - mobile: sem mudança, porque ele já lê os agentes pela visão do hub.

### Como ficou a fatia 3 (Sessões 112 e 113)

- **Decisões do usuário**:
  - um espaço é **uma pasta do vault do root**, com quem lê e quem escreve;
  - a audiência é **por pasta**, não por nota nem por etiqueta;
  - **só o root** cria espaços.
  - Consequência: espaços e audiência viram o mesmo mecanismo. O turno de um membro enxerga só o vault dele e as
    pastas do root liberadas para ele, então o root decide o que um agente sabe quando fala com outra pessoa
    mexendo em pastas, e não em instruções no prompt.
- **Config** (`warden_bootstrap::users`): `[[spaces]]` com `SpaceConfig { id, folder, readers, writers }`.
  - `id` segue a regra dos usernames. A pasta é relativa à raiz do vault, sem `..`, sem `/` no começo, sem pasta
    oculta, sem `skills/` e sem os arquivos fixos (`check_space_folder`);
  - nome e pasta únicos. As pessoas passam pelo `clean_shares` (só quem existe, ou `"*"`), e um escritor não fica
    também na lista de leitores;
  - `save_space`, `remove_space` e `spaces_for(spaces, membro)`, que devolve cada espaço com "pode escrever";
  - `remove_user` tira a pessoa de todos os espaços.
- **Montagens no `Vault`** (núcleo, `Mount { prefix, vault, writable }` e `set_mounts`):
  - com montagens, `compartilhado/` é só delas: `compartilhado/<id>/…` vai para o vault montado, relativo à raiz
    dele (o bloqueio de `..` e de caminho absoluto continua valendo lá dentro), e um `<id>` sem montagem é recusado;
  - `read`, `write`, `delete`, `read_note`, `save_note` e `delete_note` seguem a rota. A escrita num espaço só de
    leitura é recusada ("read-only for you");
  - a listagem e as buscas (texto e semântica) somam os montados com o prefixo. O `list_all_files`, que o sync usa,
    **não** soma: a pasta continua sendo do root e sincroniza com o vault dele;
  - um vault sem `set_mounts` (o do root) não muda nada: `compartilhado/` é uma pasta como outra qualquer.
- **No hub** (`people.rs`):
  - `SpaceVaults`, um `Vault` por pasta compartilhada, guardado enquanto o hub vive. Assim o modelo semântico de uma
    pasta carrega uma vez, e não a cada turno. Fica no `ConnectionContext` e no `ApiContext`;
  - `mount_member_spaces(membro, config, vault do root, cache)` refaz as montagens pelo config de agora. É chamada
    antes de cada turno de membro (junto com as tools), antes de cada pedido de vault e de skills (`person_vault` no
    `server.rs`) e em cada chamada da Warden API pela chave de um membro. Um espaço dado ou tirado vale na próxima
    mensagem, sem reconectar.
- **Protocolo**: `SpaceDto`, `ListSpaces` (todos para o root; para um membro, só os dele, com `folder` já como
  `compartilhado/<id>` e sem mostrar quem mais está), `SaveSpace` e `DeleteSpace` com a chave de pareamento,
  respondidos por `SpaceList` ou `UserError`. Um membro que tenta salvar é recusado pelo `member_refusal`.
- **Telas e CLI**:
  - web: a seção "Espaços compartilhados" na aba Pessoas (`SharedSpacesSection`), com as pastas do vault como
    sugestão e, por pessoa (e para "todo mundo"), "Não vê", "Só lê" ou "Lê e escreve";
  - desktop: "Shared spaces" dentro da seção People do Workspace (`list_shared_spaces`, `save_shared_space` e
    `remove_shared_space`), pelo `config.toml` local e sem pedir a chave, como o resto da seção;
  - CLI: `warden-server spaces list|add <nome> --folder <pasta> [--reader <u>]… [--writer <u>]…|remove`. O `add`
    de um nome que já existe muda o espaço;
  - o membro vê os espaços no próprio vault (a aba Vault da web e o celular), sem tela nova.
- **Fora desta fatia**: o agente do root não enxerga o vault do membro (nem precisa, os espaços são do root); não
  há espaço criado por membro nem pasta do vault de um membro compartilhada com outro; as skills de um espaço não
  são carregadas (`skills/` não pode ser espaço); e (até o P94, que acabou com a memória fixa) a memória fixa do root nunca entrava.

### Como ficou a fatia 4, parte A (Sessão 114)

A fatia 4 foi dividida por escolha do usuário. A **parte A** (esta) é a criptografia do que é de cada membro, o
backup e a política "privado de verdade". A **parte B** fica para depois: Shamir 2 de 3 ("recuperável com
consentimento") e recuperação de empresa com registro e aviso.

- **Decisões do usuário**:
  - a chave abre no login e **fica só na memória do hub**: token e chave da Warden API funcionam enquanto o hub
    está de pé, e depois de reiniciar o membro entra uma vez com a senha;
  - cifrar o vault (conteúdo **e nomes de arquivo e de pasta**), as conversas e os arquivos gerados.
    Os arquivos gerados entraram numa segunda passada da mesma sessão (veja "Documentos gerados");
  - o recorte em duas partes.
- **A chave** (`warden_bootstrap::member_crypto` e `users.rs`):
  - uma chave aleatória de 32 bytes por membro. Ela nunca vai para o disco em claro: o `[[users]]` guarda a
    chave **embrulhada duas vezes** (`key.by_password` e `key.by_recovery`, em base64), e qualquer uma das duas a
    abre. O embrulho é AES-256-GCM sob uma chave derivada por Argon2id (a senha, ou o código de recuperação) com sal
    próprio. O `config.toml` sincroniza, então os embrulhos acompanham o membro entre as máquinas do root, mas sem
    a senha ou o código ninguém abre;
  - **ela nasce da senha do próprio membro, nunca da provisória**, senão o root abriria o vault. Membro novo: na
    troca de senha obrigatória. Membro de antes da fatia 4, com dados em claro: no primeiro login com a senha
    própria, com migração;
  - troca de senha normal abre a chave com a antiga e embrulha com a nova, mantendo chave e código;
  - **reset do root**: `reset_password` marca `key_needs_recovery`. O root não abre a chave, então o membro entra com
    a provisória e a troca de senha passa a exigir o **código de recuperação**, que abre o segundo embrulho. Sem o
    código, os dados são irrecuperáveis: é a política "privado de verdade";
  - o **código de recuperação** são 160 bits em base32 (`ABCD-EFGH-…`), mostrado uma vez. O membro pede outro
    (`RegenerateRecoveryCode`, com a senha) e o antigo deixa de valer.
- **Chave em uso**: uma tabela no processo (`member_crypto`, por pasta) que o login preenche. Fica por pasta, e não
  por conexão, para as funções de conversa (que recebem só um `Path`) acharem a chave sem mudar 20 chamadas. Uma
  pasta com a marca `.encrypted` e sem chave na tabela está **trancada**: vault e conversas respondem
  "your data is locked…", nada em claro é escrito ao lado dos arquivos cifrados (`Vault::new_locked`), e o
  `HelloAck` diz `locked`. Entrar com a senha abre. Remover o membro trava.
- **`Vault` cifrado** (`warden-core`, `memory/cipher.rs`): `Vault::new_encrypted(raiz, cifrador)`.
  - conteúdo: AES-256-GCM, nonce novo a cada escrita, formato `WRD1 + nonce + cifra + tag`. Duas subchaves saem da
    chave do membro por HKDF (conteúdo e nomes);
  - nomes: AES-256-GCM-SIV com nonce fixo, de propósito: um nome precisa dar o mesmo texto sempre, porque um
    caminho é achado cifrando-o, e o SIV é o modo que aguenta nonce repetido (só revela que dois nomes são iguais).
    Cada pasta e cada arquivo é cifrado à parte e escrito em base32 minúsculo com prefixo `w1-`. Um nome tem no
    máximo 120 bytes;
  - `path_of` vira o mapa caminho legível → caminho no disco. Toda listagem decifra os nomes e ignora o que não é
    cifrado. Busca de texto, busca semântica (o índice `.warden/semantic_index.json` também vai cifrado),
    `read_note`/`save_note` (a versão continua sobre o texto claro) e a escrita atômica passam pela mesma camada;
  - as skills faziam IO direto no disco e foram para dentro do `Vault` (`files_in`, `is_file`, `remove_dir_all`,
    `remove_dir_if_empty`);
  - as montagens da fatia 3 seguem iguais: `compartilhado/<id>/` vai para o vault do root, em claro, que é do root.
- **Conversas**: `save_conversation`, `load_conversation` e `list_conversations` cifram e decifram pelo estado da
  pasta. Um arquivo em claro numa pasta cifrada é lido como está (é um que a migração ainda não alcançou). O nome
  do arquivo é o id da conversa, que o cliente escolhe, e **continua legível**; o título e as mensagens não.
- **Migração** (`encrypt_member_data`, idempotente): cifra conteúdo e nome de cada arquivo do vault e cada conversa,
  pulando o que já está cifrado, e só no fim grava a marca `.encrypted`. Um `.encrypting` fica enquanto roda, então
  uma migração interrompida é retomada no próximo login. Um nome longo demais fica de fora e é listado.
- **No hub**: `open_member_data_at_sign_in` (no `Hello` com senha, **antes** de a conexão ganhar o vault: abre a chave
  ou, para um membro de antes, cria a chave, migra e manda o código no `RecoveryCode` com `request_id` 0 logo após o
  `HelloAck`) e `handle_change_password` (cria ou reabre a chave e refaz o `MemberSpace` da conexão, que tinha
  nascido com o vault em claro).
- **Só com cliente que mostra o código**: o `Hello` ganhou `recoveryCodes`. O hub só liga a criptografia de alguém
  vindo de um cliente que diz que mostra o código, porque um código que ninguém vê é uma chave que ninguém tem. A
  web diz que sim. O **celular não diz** (o app em Flutter não mostra o código e não deu para mudá-lo nesta
  sessão), então um membro que só usa o celular continua sem criptografia até entrar pela web.
- **Protocolo**: `UserInfoDto` ganhou `encrypted`, `needsRecovery` e `locked`; `ChangePassword` ganhou
  `recoveryCode`; `PasswordChanged` ganhou `recoveryCode` (o código de uma chave recém-criada);
  `RegenerateRecoveryCode` e `RecoveryCode`.
- **Backup** (`warden-server backup --out <pasta> [--user <id>]`, `restore <pasta> [--user] [--force]`,
  `member_backup.rs`): como o disco já é cifrado, o backup é uma cópia do vault e das conversas do membro mais um
  `members.toml` com o `[[users]]` dele (a chave embrulhada pela senha e pelo código). Fica ilegível para o root e
  volta em outra máquina, ou depois de remover o membro por engano, abrindo só com a senha ou o código. Membro sem
  chave ainda é deixado de fora e dito. O `restore` recusa sobrescrever dados, ou trocar um membro que tem outra
  chave, sem `--force`.
- **Remover um membro** (P111, segunda passada): se os dados dele são cifrados, o `[[users]]` vai para
  `[[removed_users]]` em vez de sumir, com o embrulho da chave, e os arquivos ficam no disco. Nada se perde:
  `warden-server users removed` lista, `users restore <id>` traz de volta com a mesma senha e os mesmos dados (os
  aparelhos foram revogados, ele pareia de novo; os agentes próprios e os espaços saíram com a remoção), e
  `users purge <id> --yes` apaga os dados e a chave de vez. Enquanto um membro está arquivado, o nome dele não pode
  ser usado por outra pessoa (`add_user` recusa), para ela não herdar a pasta. Quem não tinha dados cifrados some
  como antes.
- **Documentos gerados** (P110, segunda passada): `generate_document` é uma tool padrão de membro e era criada uma
  vez com a pasta do root. Ganhou o gancho `Tool::with_media_root`, chamado pelo `Orchestrator::with_media_root`
  (as sub-tarefas de `delegate_task` também): o turno de um membro grava em `users/<id>/generated`. O `with_vault`
  da tool leva a cifra do vault do membro, e o arquivo (txt, pdf ou xlsx) é montado na memória e gravado já
  cifrado, sem passar pelo disco em claro. A mídia grande de MCP (`spill_oversized_media`) usa a mesma cifra. A
  migração cifra o que já houvesse em `generated/`, e o backup agora leva a pasta. Os **nomes** dos arquivos
  gerados continuam legíveis (o modelo os escolhe) e um cliente não tem como baixar um documento gerado de membro
  (já era assim: só o desktop do root abre o caminho).
- **Telas**: web: `RecoveryCodeView` (o código aparece uma vez e a tela só sai depois de "guardei"), o campo do
  código na troca de senha depois de um reset, "gerar um novo código" e o aviso de dados trancados; a aba Pessoas
  mostra o estado de cada membro (sem criptografia, cifrado, precisa do código). O desktop não mudou: o estado só
  aparece no `warden-server users list`.
- **Fora desta parte**: Shamir e recuperação de empresa (parte B); backup agendado; o extrato de gasto; o `shell`
  de um membro (se o root liberar) enxerga o disco cifrado; a chave sobreviver a um reinício do hub sem a senha; o
  nome do arquivo de conversa; o relatório de uso do root perde a linha de um membro trancado; e o celular (acima).

### Como ficou a fatia 4, parte B (Sessão 114)

As duas políticas que faltavam, para o root poder ajudar quem perdeu a senha e o código sem poder ler o que é dos
outros. A parte A já era a política `private`.

- **Decisões do usuário**: **duas chaves em vez de Shamir literal** (o "2 de 3" do desenho equivale a isto, porque a
  senha já abre a chave direto; sem dependência nova e sem código de criptografia delicado); a chave privada do
  root **só com o root**; a política **pode mudar depois**, cada membro é avisado, e uma mudança para uma mais fraca
  precisa do aceite da pessoa.
- **As três políticas**, uma por workspace (`recovery_policy` no `config.toml`, `private` por padrão):
  - `private`: como na parte A;
  - `consent`: o root **e** o código da pessoa, juntos;
  - `company`: o root sozinho, com o registro e o aviso.
  Força: `private` > `consent` > `company`.
- **A chave do root** (`warden_bootstrap::recovery`): um par secp256k1 (o ECIES que o `warden-truthid` já tem).
  O hub guarda só a **pública** (`recovery_public_key`), o que basta para preparar os dados de cada membro; a
  **privada** aparece uma vez, em texto para anotar (base32 em grupos), e é digitada a cada recuperação. Quem só tem
  o disco do hub não recupera nada. Uma política que precisa da chave, sem chave configurada, vale como `private`.
- **O que cada política grava** (`KeyWraps`, com `policy`, `by_escrow` e `escrow_id` novos, tudo `serde(default)` e
  sem rastro em quem está em `private`):
  - `company`: como o `private`, mais `by_escrow` = a chave do membro selada para a pública do root;
  - `consent`: o `by_recovery` passa a ser a chave já selada para o root, embrulhada pelo código. O código sozinho
    deixa de abrir (dá um blob que só a privada decifra) e o root sozinho também.
- **Aplicar a política a um membro** (`sync_recovery_policy`) só dá com a chave aberta, ou seja, **no login com a
  senha** ou ao aceitar. Igual ou mais forte aplica sozinho; **mais fraca fica pendente** até a pessoa aceitar
  (`AcceptRecoveryPolicy`, com a senha, que abre a chave). Entrar ou sair de `consent` (ou trocar a chave do root
  dentro dele) gera um **código novo**, porque o embrulho do código muda de forma e o antigo não está à mão; ele vem
  no `RecoveryCode` de sempre, e só para um cliente que mostra o código (`recoveryCodes`). Um membro novo já nasce
  na política do workspace. `--new-key` troca o `escrow_id` e todos se refazem no próximo login.
- **Recuperar** (`recover_member`): `company` só com a chave; `consent` com a chave e o código da pessoa (que ela
  entrega ao root); `private` recusa, e diz que nem o root consegue. Abre a chave, põe uma **senha provisória nova**
  (mostrada uma vez ao root) que a embrulha, e a pessoa entra com ela e escolhe a própria. Embrulhar pela provisória
  não expõe nada a mais do que o root acabou de poder fazer. O código de recuperação não muda. Depois de um
  `ResetPassword` de um membro em `consent`, o código sozinho não recupera: a troca de senha manda pedir ao root.
- **Registro e aviso**: cada recuperação vira um `RecoveryEvent { at_ms, kind, seen }` em `UserConfig.recoveries`.
  A pessoa vê no `HelloAck` (`recoveries`) e uma tela de aviso ao entrar, com "Entendi" (`AckRecoveryNotices`); o
  histórico fica. O root vê a contagem e a data na aba Pessoas e em `warden-server recovery log`.
- **Protocolo**: `UserInfoDto` ganhou `memberPolicy`, `policyPending`, `recoveryPolicy` (só no `HelloAck`) e
  `recoveries`; `AcceptRecoveryPolicy` → `RecoveryPolicyAccepted` (com o código novo, se houver),
  `AckRecoveryNotices` → `RecoveryNoticesAcked`, `SetRecoveryPolicy` → `RecoveryPolicy` (com a chave privada só
  quando acabou de ser feita), `RecoverMember` (responde `UserList` com a senha provisória) e `recoveryPolicy` no
  `UserList`.
- **CLI**: `warden-server recovery policy [private|consent|company] [--new-key]`, `recovery recover <id> --key
  <chave> [--code <código>]` e `recovery log`.
- **Web**: na aba Pessoas, a seção "Recuperação dos dados" (`RecoveryPolicySection`: a política, a chave mostrada
  uma vez, e "Recuperar" por pessoa com a chave e o código); para o membro, `RecoveryNoticeView` (a política que
  mudou, com o aceite, e as recuperações que o root fez). O desktop só preserva os campos ao salvar.
- **Honestidade** (está no texto das telas): o registro e o aviso protegem contra o uso descuidado e contra os
  membros, não contra quem controla a máquina do hub e edita o `config.toml`. `company` entrega ao root o poder de
  abrir os dados de todos, com registro, e a pessoa aceita isso ao confirmar.
- **Fora desta parte**: Shamir literal; o celular (não mostra o código nem o aviso, P109); botão de restaurar
  membro removido na web; aprovação de mais de uma pessoa; e um grupo de membros com chave própria.

## Tarefas agendadas (P92, desenho da Sessão 108)

Conversa de desenho com o usuário. Hoje não existe agendador: os jobs (P46) vivem na memória de um turno. A ideia
é o Warden fazer coisas sozinho ("todo dia às 8h resume meus e-mails", "a cada hora confere tal coisa"), com um
agente, as tools dele e os limites de gasto do P4, num hub que fica sempre de pé.

| Pergunta | Decisão |
|---|---|
| Onde a tarefa é definida | `[[tasks]]` no `config.toml`, como os agentes (o estudo do P86 já previa "definição = mapa"): `id`, `agent`, `prompt`, `schedule`, `enabled`. Sincroniza junto com o resto da configuração |
| Formato do agendamento | Três formas: `every = "1h"` (intervalo), `cron = "0 8 * * 1-5"` e `once = "2026-10-01T09:00"`, no fuso configurado. O agente converte "todo dia às 8h" para uma delas |
| Estado de execução | Última execução, próxima e o resultado ficam num arquivo do hub, **fora do sync**: é do nó que executa, não do workspace |
| Para onde vai o resultado | **Uma conversa da tarefa** (escolha do usuário), numa área do hub compartilhada entre os aparelhos (as conversas do hub hoje são por aparelho, `<root>/<device>/`). Cada execução vira uma mensagem nela, e os aparelhos conectados recebem aviso. No celular é a notificação local da Fase 7.5 (sem push de verdade), então só chega com o app vivo |
| Hub desligado na hora | **Roda uma vez ao voltar** (escolha do usuário), mesmo que tenha perdido várias horas marcadas. Uma tarefa `once` que passou roda ao voltar e depois se desliga |
| Tool que pede aprovação | **Recusada** (escolha do usuário), igual Telegram e WhatsApp: ninguém está olhando. O agente segue sem ela e o resultado diz o que não conseguiu fazer |
| Qual hub executa | **Uma chave local por hub**, "executar tarefas agendadas neste hub", fora do sync e desligada por padrão (escolha do usuário). Como o `config.toml` sincroniza, sem ela o hub do VPS e o hub embutido do desktop rodariam a mesma tarefa duas vezes |
| Gasto | Valem os limites do P4 e as `allowed_tools` do agente escolhido. Sem limite próprio por tarefa por enquanto |
| Agente criando tarefas | Tool `manage_tasks`, só para agentes com `can_manage_tasks` (ligado por uma pessoa), e toda criação ou mudança espera o sim do usuário. Mesmo padrão do `manage_agents` |
| "Pode repetir" vs "no máximo uma vez" | Fica para o P86. Com um único hub executando, o campo não mudaria nada |

**Fatias** (escolha do usuário):

1. O motor no hub (`warden-server` e hub embutido do desktop), `[[tasks]]`, a conversa da tarefa, a chave por hub
   e `warden-server tasks` (listar, criar, pausar, rodar agora). **Feita na Sessão 108**, menos a chave do hub
   embutido do desktop, que precisa de tela e foi para a fatia 2.
2. Telas na web e no desktop, e a chave "executar tarefas" do hub embutido do desktop.
3. A tool `manage_tasks`, para criar tarefas em linguagem natural.

### Como ficou a fatia 3 (Sessão 108)

- **`manage_tasks`** (`warden-bootstrap/src/manage_tasks.rs`), no molde do `manage_agents`: `list` (a hora e o
  offset do hub, as tarefas com próxima e última execução, e os agentes que quem chama pode agendar), `create`,
  `update` (só o que for dado; um agendamento novo substitui o anterior; `enabled` pausa ou retoma) e `delete`.
  Sem "rodar agora": para isso o agente já trabalha na própria conversa.
- **Flag `can_manage_tasks`** no `AgentConfig`, ligada só por uma pessoa (Settings da web e do desktop, `/agents`
  no CLI). O `manage_agents` nunca a liga, preserva numa edição, e `manage_tasks` não pode entrar numa lista de
  tools. A tool é anexada no `scope_to_agent`, o que cobre o hub (web, mobile, extensão), o desktop e o CLI de uma
  vez, com a aprovação pelo mesmo caminho do `manage_agents`.
- **Regras na tool**:
  - pedido impossível (agendamento inválido, agente inexistente, id repetido) é recusado antes de perguntar;
  - o cartão mostra agente, agendamento com fuso, próxima execução e o prompt inteiro (no `update`, antes e depois);
  - sem approver recusa, então uma execução agendada não cria tarefas sozinha;
  - **ninguém passa mais do que tem**: um agente com `allowed_tools` só agenda agentes cujas tools caibam nas dele,
    e nunca uma tarefa sem agente (todas as tools), um agente sem limite ou um que alcança outros agentes ou muda
    configurações (qualquer flag `can_*`).

### Como ficou a fatia 2 (Sessão 108)

- Decisões do usuário: no desktop, o resultado aparece **na tela de Tarefas** (a última resposta e o histórico em
  modo leitura), não no chat; na web, **toda mudança pede a chave de pareamento**, como as chaves da API.
- **Chave local**: `HubLocalConfig { run_tasks }` em `<config dir>/warden/hub-local.json`. Não pode ser campo do
  `config.toml`, que sincroniza inteiro (inclusive `embedded_server`): a chave ligaria em todas as máquinas ao
  mesmo tempo. O `warden-server` avulso segue com `--run-tasks`. Mudar a chave no desktop reinicia o hub embutido
  (`restart_embedded_server`, com novas tentativas enquanto a porta é liberada).
- **Helpers compartilhados** em `tasks.rs`: `upsert_task` (cria, edita ou renomeia, limpa campos vazios do
  formulário e confere a lista inteira), `remove_task`, `set_task_enabled`, `task_status`/`task_infos` e as
  conversões com `TaskDto`. O `tasks list` do CLI usa o mesmo `task_status`.
- **Protocolo**: `ListTasks`, `SaveTask`, `SetTaskEnabled`, `DeleteTask`, `RunTask`, todas respondidas por
  `TaskList { tasks, runsHere }` ou `TaskError { authRejected }`. `TaskInfoDto` é a tarefa mais o estado (próxima
  e última execução, erro, `running`).
- **Hub** (`task_admin.rs`): listar é aberto a qualquer aparelho pareado. Cada mudança confere a chave sob o
  `settings_lock`, com a mesma espera de 1 s. Mexe só no `[[tasks]]`, sem recarregar o orquestrador. O
  `TaskRunner` (em `scheduler.rs`, criado no `with_tasks`) é o mesmo para o laço e para o "rodar agora", então as
  duas execuções da mesma tarefa nunca se sobrepõem. `Server::task_runner()` o expõe ao desktop.
- **Web**: a aba **Tarefas** (`TasksView.tsx`), com a lista, o formulário (agendamento a cada / cron / uma vez, e
  o fuso do navegador sugerido), "Abrir conversa" (o chat em `task-<id>`) e o pedido de chave. Recarrega quando
  chega `ConversationsChanged` de um `task-*`.
- **Desktop**: a tela **Tasks** (`TasksView.tsx`, `task_cmds.rs`) com a chave "Run scheduled tasks on this
  computer", as mesmas ações sem pedir chave, e a última resposta e o histórico lidos da pasta local. O "Run now"
  usa o `TaskRunner` do hub embutido quando ele está de pé (os aparelhos dele ficam sabendo); senão, um runner
  local. A tela consulta o estado a cada 3 s enquanto alguma tarefa está rodando.

### Como ficou a fatia 1 (Sessão 108)

- **`warden-bootstrap/src/tasks.rs`**: `TaskConfig` (`id`, `agent`, `prompt`, um entre `every`/`cron`/`once`,
  `timezone`, `enabled`), `check_tasks`, o cálculo de horário e o `run_task`. Cron de 5 campos com o crate
  `croner` (sem segundos nem ano), fuso IANA com o `chrono-tz`; sem `timezone`, vale o fuso da máquina do hub.
  `every` aceita `m`, `h` e `d`, no mínimo 1 minuto, e conta a partir da última execução.
- **Estado** em `<config dir>/warden/tasks-server/state.json`, com a mesma trava de arquivo das conversas (o hub e
  um `tasks run` em outro processo). Por tarefa: quando o hub a viu, quando rodou, quando terminou e o erro. A
  âncora do próximo horário é a última execução ou, se nunca rodou, quando o hub a viu, então uma tarefa nova não
  dispara na hora. Mudar o agendamento (não o prompt nem o agente) zera o estado. **Uma tarefa pausada conta a
  partir de quando volta**, sem compensar o que pulou. Um `once` que rodou não roda mais, sem reescrever o config.
- **A execução** usa o orquestrador do hub escopado ao agente (`scope_to_agent`, com o modelo dele), gasto no canal
  `tasks` com o usuário `task:<id>`, **sem approver** e sem `message_agent`. A mensagem enviada é
  `[Scheduled task '<id>', <data e hora no fuso da tarefa>]` mais o prompt, para o modelo saber que dia é. **Só as
  últimas 20 mensagens da conversa vão como histórico** (10 execuções); a conversa guarda tudo. Um erro vira nota na
  conversa, como no `message_agent`.
- **Conversas das tarefas** em `<config dir>/warden/tasks-server/conversations/`, fora de `conversations-server/`
  (as subpastas de lá são ids de aparelho, e um aparelho chamado "tasks" colidiria). No hub, `ConversationDirs`
  manda todo id `task-*` para essa pasta: a lista de cada aparelho junta as duas, e histórico, renomear, apagar e
  `Chat` funcionam nela. Dá para continuar o assunto na conversa da tarefa. O relatório de gasto ganha uma linha
  "Tarefas agendadas" (hoje "Tarefas e webhooks", P105: as conversas dos webhooks ficam na mesma pasta).
- **No hub** (`warden-server/src/scheduler.rs`): um laço a cada 30 s que relê o config (pausar ou editar vale sem
  reiniciar), marca o que venceu e roda cada tarefa numa task própria. Uma execução que ainda não terminou faz a
  próxima ser pulada, em vez de empilhar. Ao terminar, um `broadcast` no `Server` avisa **todas** as conexões com
  `ConversationsChanged`, e a web, o mobile e a extensão já recarregam a lista com isso, sem mudança nos clientes.
  `Server::with_tasks(store, run)` deixa a API pronta para o hub embutido do desktop.
- **CLI**: `warden-server serve --run-tasks` e `warden-server tasks list|add|pause|resume|remove|run`. O `add`
  valida com `check_tasks` e salva pelo `save_config` (mantém os comentários, P82). O `run` executa no próprio
  processo; um `serve` rodando não avisa os aparelhos dessa execução, que aparece quando a lista recarregar.

## Nós como capacidades (P93, desenho da Sessão 108)

Saiu do estudo do P86: outras máquinas entram no workspace e **emprestam o que têm** (shell, arquivos, servidores
MCP, um modelo local), e o estado continua no hub. Os nós só executam. Um agente ou uma tarefa (P92) pede "roda no
nó de casa" ou "roda num nó com GPU".

**O que já existe**: o `Hello` de um aparelho já anuncia tools (o celular empresta `list_phone_files` e
`read_phone_file`, Fase 7.4), e o hub faz o proxy com o `RemoteTool`/`RemoteToolChannel`. Mas essas tools só
servem à conversa daquele mesmo aparelho. O `CallDeviceTool` (Fase 9.3/9.4) roteia entre aparelhos, exige
aparelho `Approved` e hoje não tem cliente.

| Pergunta | Decisão |
|---|---|
| O que roda no nó | **`warden-server node --hub wss://…`** (escolha do usuário): o binário que já existe conecta no hub como mais um aparelho e oferece o que foi liberado. Serve para VPS, mini-PC e servidor sem tela. O desktop ganha uma chave "emprestar este computador" numa fatia depois |
| O que oferece | Shell, arquivos de uma pasta escolhida, servidores MCP daquela máquina e o modelo local (Ollama ou outro OpenAI-compatible) como provedor no hub (as quatro, escolha do usuário), em fatias |
| Como o agente escolhe | **Tools genéricas com um parâmetro `node`** (escolha do usuário): `list_nodes` (quem está online, descrição, etiquetas como "gpu" e "casa", o que cada um oferece), `node_shell`, `node_read_file`, `node_write_file`. A lista de tools não cresce a cada nó novo. **Exceção: as tools MCP de um nó** viram tools próprias com prefixo (`casa__github_search`), com o schema delas, só enquanto o nó está online e para os agentes liberados (escolha do usuário: o modelo usa muito melhor uma tool com schema) |
| Permissão | **Duas travas, como os hosts SSH do P47** (escolha do usuário). No nó, quem instala escolhe o que ele oferece (`--shell`, `--files <pasta>`, …). No hub, cada nó tem: ligado, quais agentes podem usar (vazio = todos) e "pedir aprovação a cada chamada". O nó precisa estar aprovado na lista de aparelhos |
| Nó cai no meio de uma chamada | A chamada falha com um erro claro e **não é repetida**: um comando de shell não é idempotente. O agente decide o que fazer |
| Onde fica a permissão do hub | `[[nodes]]` no `config.toml`, pelo id do aparelho, como `ssh_hosts` |

**Fatias** (escolha do usuário):

1. O nó (`warden-server node`), `list_nodes`/`node_shell`/`node_read_file`/`node_write_file`, as permissões no hub
   e as telas básicas.
2. Os servidores MCP do nó, como tools próprias com prefixo.
3. O modelo local do nó como provedor no hub (streaming do modelo pelo WebSocket).

### Como ficou a fatia 1 (Sessão 108)

- **O nó** (`warden-server node`, `node_client.rs`):
  - `--hub`, `--auth-key` (só na primeira vez), `--name`, `--description`, `--tag` (repetível), `--shell` e
    `--files <pasta>`;
  - a identidade (id gerado uma vez e o token do hub) fica em `<config dir>/warden/node.json` (0600), então ele
    volta sozinho mesmo depois de trocarem a chave de pareamento do hub;
  - reconecta com espera de 1 s a 60 s e manda ping a cada 20 s;
  - executa localmente `shell` (o `ShellTool`, na pasta `--files` ou em home), `read_file` (texto, até 1 MB),
    `write_file` e `list_files` (até 1000), com a pasta como `Vault`, cujo `path_of` recusa `..` e caminho
    absoluto;
  - o que não foi ligado é recusado no próprio nó.
- **Protocolo**: `Hello.node: Option<NodeOfferDto>` (descrição, etiquetas, `shell`, `files`). As chamadas usam o
  `ToolCallRequest`/`ToolCallResult` que já existiam (Fase 7.4). Na web: `ListNodes` e `SetNodeAccess` →
  `NodeList`/`NodeError`.
- **No hub**:
  - `NodeRegistry` (`nodes.rs`): quem está conectado como nó e o que ofereceu por último. Sai pelo mesmo canal, e
    uma conexão velha não derruba uma nova do mesmo nó;
  - as tools (`node_tools.rs`): `list_nodes`, `node_shell`, `node_read_file`, `node_write_file` e
    `node_list_files`, genéricas com `node` (o `enum` traz os ids utilizáveis), no molde das tools de SSH
    (`scoped_to_agent`, `with_approver`, `is_available`, log em `node_audit.jsonl` sem o conteúdo dos arquivos);
  - um nó só aparece para um turno se estiver conectado, aprovado na lista de aparelhos, ligado no `[[nodes]]`,
    aberto àquele agente e oferecendo o que a chamada pede. Tudo é relido a cada chamada;
  - as tools entram por `SharedOrchestrator::set_extra_tools`, reaplicado a cada `replace` (save de settings,
    sync), então chat, Warden API e tarefas agendadas recebem as mesmas.
- **Nó que cai**: `RemoteToolChannel::close()` falha na hora as chamadas pendentes, e a mensagem diz que não foi
  repetida. **Achado no teste**: um erro de leitura na conexão (o outro lado sumiu sem fechar) saía da função pelo
  `?` antes da limpeza, e a chamada esperava o timeout inteiro. Agora o erro encerra o laço como um fechamento, o
  que também limpa qualquer aparelho que caia assim.
- **`[[nodes]]`** (`NodeAccessConfig { id, enabled, agents, require_approval }`): remover um agente pelas
  Settings da web ou pelo `/agents` tira o nome dele dos nós, e um nó que fica sem agente é desligado, nunca aberto
  a todos. Um nome que sobrar (removido por outro caminho) não casa com ninguém, então só fecha o acesso.
- **Telas e CLI**:
  - web: seção "Nós" na aba Aparelhos, com a chave de pareamento a cada mudança;
  - desktop: seção "Nodes" no Workspace, que também libera por id um nó ligado ao hub do VPS, já que o
    `[[nodes]]` sincroniza;
  - hub sem tela: `warden-server nodes list|allow <id> [--agent …] [--approval]|deny <id>`.

### Como ficou a fatia 2 (Sessão 108)

- Decisão do usuário: **o nó empresta, por nome, servidores do `[[mcp_servers]]` do `config.toml` dele**
  (`--mcp github --mcp postgres`, `--config` para outro arquivo), no formato que o desktop e o CLI já usam,
  inclusive OAuth. Nada vai sem ser nomeado. Um nome que não existe, ou um servidor que não sobe, impede o nó de
  começar: emprestar metade do pedido seria uma surpresa.
- **`connect_mcp_server`/`add_mcp_tools`** saíram do laço do `bootstrap` (que passou a usá-los) e são o que o nó
  usa para subir os servidores. Uma colisão de nome entre dois servidores no nó vira `servidor__tool`, como no hub.
- **No nó**: `LocalNode::with_mcp_tools`, e a oferta leva os schemas (`NodeOfferDto.mcp_tools`). A chamada chega
  como `ToolCallRequest { tool: "mcp", arguments: { tool, arguments } }`, e uma tool não emprestada é recusada lá.
- **No hub**, cada tool MCP de um nó online vira uma **`NodeMcpTool` própria**:
  - nome `<slug do nome do nó>__<tool>` (`casa-pc__query`): minúsculas e `-`, até 64 caracteres; numa colisão
    entre dois nós com o mesmo nome entra o fim do id;
  - a descrição começa com "On node '<nome>':", e o schema é o original;
  - as mesmas regras das tools da fatia 1 (online, aprovado, ligado, agente liberado, aprovação, log, sem repetir).
- **Tools que vêm e vão**: o `SharedOrchestrator` guarda a base (como as settings a construíram), as tools fixas
  do hub e as **dinâmicas**. Quando um nó entra ou sai, `set_dynamic_tools(factory.mcp_tools())` remonta o
  orquestrador. Um turno já em andamento fica com o que pegou no começo.
- **O log de auditoria** passou a guardar só o tamanho de `content` e de qualquer texto com mais de 200
  caracteres, nas tools da fatia 1 e nas MCP (achado no ponta a ponta: as MCP gravavam o conteúdo escrito).
- `list_nodes` mostra as tools MCP de cada nó e o prefixo delas; as telas da web e do desktop mostram quantas e
  quais o nó empresta.

### Como ficou a fatia 3 (Sessão 108)

- Decisões do usuário: **um provedor `kind = "node"`** no `[[providers]]` (o id do nó em `node`, e em `model` o id
  do provedor **no nó**), que entra em todo seletor, inclusive combos; e **a lista de agentes do `[[nodes]]` também
  limita o modelo**. A aprovação a cada chamada não vale para o modelo.
- **No nó**: `--model <id>` empresta um `[[providers]]` do `config.toml` dele (o Ollama local, por exemplo). A
  oferta leva os ids (`NodeOfferDto.models`). O nó recebe `ModelRequest { request_id, model, messages, tools }`,
  chama o provedor local e devolve `ModelEvent` (texto, tool call, uso) e `ModelDone`, ou `ModelError { transient
  }`. Um `ModelCancel` aborta a resposta. Um combo no próprio nó responde, mas o aviso de fallback de lá não
  atravessa.
- **No núcleo**:
  - `Message`, `ToolCall`, `Role` e `StreamEvent` ganharam serde (o `StreamEvent` com `kind`/`data`);
  - o erro **`ProviderUnavailable`**, que o `FallbackProvider` trata como transitório, como um 503;
  - **`ModelProvider::for_agent`**: o modelo fica sabendo quem pede, como o `scoped_to_agent` das tools. O
    `FallbackProvider` repassa aos de dentro, e o `Orchestrator` aplica em `with_agent` e também em `with_model`,
    porque os canais trocam o modelo depois de escopar o agente.
- **O provedor** (`warden-bootstrap/src/node_model.rs`): `NodeModelProvider` pede a um **roteador do processo**
  (`set_node_model_router`), que o hub instala ao subir. Sem hub no processo (o CLI, o desktop com o hub
  desligado) a chamada dá `ProviderUnavailable`, e um combo segue. O roteador é global: dois hubs no mesmo processo
  (só em teste) disputariam ele.
- **No hub** (`nodes.rs`):
  - cada nó conectado tem um `ModelChannel` (`request_id` → stream), alimentado pelo laço de leitura;
  - `HubNodeModelRouter` confere online, aprovado, ligado, agente liberado e modelo oferecido, e responde
    `ProviderUnavailable` com o motivo;
  - espera o primeiro evento antes de devolver o stream, então uma falha antes de qualquer resposta ainda deixa o
    combo seguir;
  - largar o stream antes do fim manda `ModelCancel`;
  - a queda do nó falha na hora todas as respostas abertas.
- **Validação**: um provedor `node` exige `node` e `model`. Um `node` que sobra em outro tipo (o tipo foi trocado no
  formulário) é descartado.
- **Telas**: o tipo "Modelo de um nó" nos provedores da web e do desktop (id do nó e provedor no nó, sem chave nem
  URL), e os modelos oferecidos nas listas de nós. O assistente `/models` do CLI não cria esse tipo (cadastro pelas
  telas ou no arquivo); editar um mantém o `node`.

### Desktop como nó, "emprestar este computador" (P97, Sessão 109)

- **O mesmo nó do CLI**: o desktop roda o `node_client` do `warden-server` (`LocalNode`, `run_node`), com a mesma
  identidade em `node.json`. `lend_mcp_servers` e `lend_models` saíram do `main.rs` para o `node_client`, e os
  dois lados usam a mesma validação.
- **Onde fica**: em `HubLocalConfig.lend` (`LendConfig`), no `hub-local.json`, que fica fora do sync: ligar num
  computador não liga em todos. Os campos são os do CLI: `hub_url`, `name` (vazio vira o host name),
  `description`, `tags`, `shell`, `files`, `mcp` e `models`, mais o `enabled`. **A chave de pareamento não é
  gravada**: ela só vale até o hub emitir o token, que vai para o `node.json`. Se o desktop recebe uma chave
  tendo já um token, ele pareia de novo (o hub emite um token novo). É assim que se troca de hub.
- **O que o motor ganhou** (vale para os dois lados):
  - `NodeActivity`: as últimas 200 chamadas (`ActivityEntry { at, kind, summary, error }`), só na memória. O
    `summary` leva o comando, o caminho, a tool MCP ou o modelo, nunca o conteúdo de um arquivo. Uma resposta de
    modelo cancelada pelo hub não entra;
  - `NodeState` (`connecting`, `connected`, `retrying { error, in_secs }`, `stopped { error }`) num
    `tokio::sync::watch`. O `run_node` recebe `Option<watch::Sender>`, e o CLI passa `None`;
  - **ser recusado para de tentar**: o `ServerConnection` agora devolve um erro tipado `AuthRejected` (chave
    errada, token revogado, `AuthError` no meio da conexão), e o `run_node` para em vez de bater no hub para
    sempre. Isso vale também para o `warden-server node`.
- **Hub embutido ao mesmo tempo**: os dois convivem, porque o desktop pode ser hub da rede de casa e emprestar ao
  VPS. Emprestar ao próprio hub embutido (`localhost`, `127.0.0.1` ou `::1` na porta dele, ligado ou não) é
  recusado, porque os agentes dele já têm este computador. Outra porta na mesma máquina é outro hub e passa.
- **Ao abrir o app**: se estava ligado, religa (`lend_cmds::restore_lending`), depois do hub embutido. Uma
  falha (por exemplo, sem pareamento) fica como `stopped` na tela, sem travar a abertura.
- **Tela**: a seção "Lend this computer" no Workspace, acima de Nodes. Tem o formulário (MCP e modelos marcados a
  partir do `config.toml` desta máquina, sem os provedores `kind = "node"`), a linha de estado com o device id e
  a lista "What the agents did here", atualizada a cada 3 s enquanto está ligado. O formulário trava enquanto
  empresta.
- **Limite conhecido**: o `warden-server node` e o desktop na mesma máquina dividem o `node.json`, então são o
  mesmo nó para o hub. Os dois ligados ao mesmo tempo disputariam a conexão.

### Como ficou a fatia 5, convite e vínculo do TruthID (Sessão 115)

- **Escopo**: o root gera um convite e a pessoa liga o TruthID dela ao usuário. **Não há login por TruthID ainda**
  (P113): o app TruthID só entrega a resposta num `https://` com certificado válido, e o hub não tem essa URL.
- **Convite**: `Invite { secret_hash, expires_at }` no `[[users]]`. O código é `<usuário>:<segredo>` (20 caracteres
  sem letras parecidas), mostrado uma vez; só o hash Argon2 do segredo fica no disco. Vale 7 dias, serve uma vez, e
  um novo substitui o aberto. Todo erro (usuário inexistente, segredo errado, usado, expirado) dá a mesma resposta,
  e o caminho do usuário inexistente também gasta um hash.
- **Vínculo**: `TruthIdLink { username, identity_id, linked_at }`. Quem resgata é um membro **já logado**, só com o
  próprio convite, e informa o username do TruthID; o hub consulta `getIdentity(username)` no `IdentityRegistry` da
  Base (`warden-truthid::identity`, `eth_call` com a ABI codificada à mão, sem dependência de Ethereum) **sem segurar
  o lock** das configurações, e só depois usa o convite. Uma identidade não liga a dois membros.
- **O que o vínculo não prova**: que a pessoa controla aquele TruthID. Digitar um username alheio não dá nada,
  porque o login futuro vai exigir a assinatura de um aparelho daquela identidade (`DeviceRegistry.getDevice` →
  `identityId`, que é o `identity_id` guardado aqui).
- **Configuração**: `truthid_network` (`base-mainnet` por padrão, ou `base-sepolia`) e `truthid_rpc_url` (o RPC
  público da rede por padrão) no `config.toml`.
- **Protocolo**: `CreateInvite` e `UnlinkTruthId` (root, com a chave de pareamento), `RedeemInvite` (membro),
  `TruthIdLinked`, `invite_code` no `UserList`, e `truthid` e `invite_open` no `UserInfoDto`.

### Login por TruthID (Sessão 115, P113)

- **Fluxo**: o navegador abre o WebSocket e manda um `Hello` com `truthidLogin` (sem chave, senha nem token). O hub
  cria um desafio `{type, nonce, issuedAt, origin}` (nonce UUID v4, `origin` é o host de `truthid_public_url`), responde
  `TruthIdChallenge` com o JSON do QR e **espera**. O app TruthID assina o JSON do desafio (`personal_sign`), cria a
  sessão on-chain e posta `{approved, nonce, signature, deviceAddress}` em `POST <truthid_public_url>/auth/truthid`
  (rota do `serve_web_or_ws`, `truthid_login::handle_callback`). O hub entrega o resultado à conexão que espera, que
  segue como um login por senha (`PairingProof::Member`): token de aparelho e `HelloAck`.
- **Conferência** (`warden-truthid::login` e `identity`): aprovado, dentro de 2 minutos (o app só aceita escanear em
  30 s, mas cria a sessão on-chain depois), o mesmo nonce, a assinatura recuperada (`k256`, keccak do prefixo
  `\x19Ethereum Signed Message:\n`) igual ao `deviceAddress`, `DeviceRegistry.getDevice` com o aparelho existente e não
  revogado, e um membro cujo `truthid.identity_id` é o `identityId` do aparelho. Uma resposta só tem uma chance: o
  desafio sai da tabela antes de qualquer conferência. Toda recusa responde só "invalid" e o motivo vai para o log.
- **Limites**: no máximo 32 desafios esperando; o QR vale 30 s (a página pede outro) e o hub espera a resposta por 2 min.
- **Verificação contra o código real**: o vetor de assinatura dos testes veio do `signChallenge` do app (web3dart) e é
  lido de volta pelo `recoverPersonalSignatureAddress` do SDK Dart; a assinatura é determinística e a nossa sai
  idêntica byte a byte.
- **O que não faz**: não abre a chave de dados (ver o P113); não há login do dono por TruthID; só a web tem a aba.

### Fim da memória fixa do vault (Sessão 115, P94)

- **O que saiu**: `FIXED_VAULT_FILES`, `Vault::standing_memory` (o bloco "Standing memory from the user's vault" que o
  `Orchestrator` punha depois da persona em toda mensagem), `seed_default_vault_files` (o `bootstrap()` não cria mais
  os 3 arquivos), `is_fixed_vault_file` (a busca de texto e a semântica e a árvore deixam de pular os nomes
  reservados) e o painel "Memória fixa" das telas do Vault (web e desktop). Isso substitui as decisões do P52 acima.
- **Por quê**: um perfil fixo por vault não serve a um workspace com várias pessoas. O vault de cada membro nascia com
  os mesmos 3 arquivos, e a memória fixa do root nunca entrava num espaço compartilhado.
- **O que ocupa o lugar** (escolha do usuário): nada. O que é permanente (quem é a pessoa, como a IA deve agir) vai
  na persona do agente, que já existe e é por agente; o resto fica em notas comuns, que entram pelo contexto da busca
  quando são relevantes.
- **Vaults que já tinham os arquivos**: nada é movido nem apagado; eles viram notas como as outras. Por isso o conteúdo
  deles **deixa de entrar automaticamente** em toda conversa, e vale copiar o que importa para a persona.

### O assistente aprendendo com as conversas, fatia 1 (Sessão 115, P104)

- **Base**: o estudo do Hermes (`STUDIES.md`). Os dados dele mostram que a revisão por relógio gera lixo (sobretudo de
  memória), que as skills são o que rende, e que skills escritas pelo próprio agente precisam de governança. Daí: sem
  memória automática (o P94 acabou com os arquivos fixos), e o aprendizado vira **skill sugerida**.
- **`search_history`** (`warden-bootstrap/src/history.rs`): o agente busca no histórico da própria pessoa por palavras
  (3 letras ou mais), ordenado por quantas casam e depois pela recência, no máximo 20 trechos. A pasta de conversas vem
  de um gancho novo `Tool::with_conversations_dir` / `Orchestrator::with_conversations_dir` (irmão do `with_media_root`):
  o `bootstrap()` registra a tool, e o hub a aponta para a pasta de quem fala (`member_orchestrator`, o caminho do root e a
  Warden API). Um membro tem a tool por padrão (`default_member_tool`) e nunca alcança a pasta de outro.
- **Skill sugerida**: um arquivo `skills/<nome>.md` com `proposed: true`, `source` (a conversa) e `proposed_at` no
  frontmatter. O catálogo, o `use_skill` e o `read_skill_file` a tratam como inexistente; ela vive no vault de quem aprendeu
  (cifrado para um membro, só dele). **Aceitar** é salvar sem a marca (o `SaveSkill` que existia); **rejeitar** é apagar.
- **O ciclo** (`warden-bootstrap/src/learning.rs`): depois de o hub entregar a resposta de um turno, e só com `[learning]
  enabled = true` (padrão desligado), (1) um **detector** de uma chamada curta olha o último trecho da conversa (com mais 4
  mensagens só de contexto) e responde `correction`, `discovery` ou `none`; (2) com sinal, uma segunda chamada lê as últimas 12
  mensagens e as skills que já existem e devolve uma skill ou `null`. O texto da conversa vai nos prompts como dado, marcado
  como não confiável. A proposta é validada (nome slug, descrição de até 300 caracteres, corpo de até 4 KB, sem arquivos),
  nunca sobrescreve um nome existente (ganha `-2`, `-3`) e nasce restrita ao agente que falava.
- **Freios**: `max_per_day` (padrão 3) sugestões por pessoa em 24 h, no máximo 10 esperando resposta, e `[learning]
  provider` para usar um modelo barato.
- **Gasto**: as chamadas passam por `Orchestrator::one_shot`, que respeita e registra o limite da pessoa (uma pausa por limite
  só pula o aprendizado, sem chamar o modelo) e nunca falha o turno: o que dá errado vai para o log.
- **Telas**: a web ganhou "Sugeridas pela IA" na aba Skills (texto inteiro, origem, Aceitar, Editar com a opção de aceitar,
  Rejeitar); o desktop mostra uma etiqueta e salvar lá aceita.

### O assistente aprendendo, o resto do P115 (Sessão 116)

- **Sugestões em todas as telas**: o `SkillDto` do celular (bridge, frb regenerado), o desktop e a extensão passaram a
  carregar `proposed`/`source`/`proposed_at`/`revises`; cada um tem a seção de sugeridas com Aceitar, Editar e Rejeitar, e
  **editar e salvar mantém a sugestão pendente** (aceitar é um salvar explícito sem a marca). Antes, salvar no celular
  aceitava sem a pessoa saber.
- **Opt-out por membro**: `UserConfig.learning_opt_out` e `SetLearning` (o membro, sobre si; o dono recebe `UserError` e usa o
  `[learning]` do arquivo). Regra única em `users::learning_allowed`: workspace ligado **e** membro não optou por sair, relido a
  cada turno. `helloAck` leva `learningEnabled`/`learningOptOut`; a web tem o checkbox na tela de Skills.
- **Onde roda o aprendizado**: `learning::learn_with_config` é o ponto único (regra do `[learning]`, opt-out, modelo,
  canal de gasto, scanner, log) e é chamada pelo hub, pelo Telegram, pelo WhatsApp e pelo desktop (`append_conversation_messages`
  dispara `learn_in_background` quando a troca termina na resposta da IA). **Fora, por decisão**: o CLI (não guarda
  conversa) e a Warden API (sem estado).
- **`manage_skill patch`**: troca um trecho único das instruções (`old_string`/`new_string`/`replace_all`), mantendo descrição,
  agentes e anexos. Nenhuma ação do `manage_skill` mexe numa skill **sugerida** pendente (um `update` a aceitaria).
- **Revisões**: quando a lição cabe numa skill ativa, o estágio 2 devolve `{"revise": ...}` (recebe o texto das ativas, até 10
  de 1500 caracteres) e grava `<skill>-revision` com `revises:` no frontmatter. **Aceitar aplica no alvo dentro de
  `SkillStore::save`**, então nenhum cliente muda o fluxo; salvar ainda pendente preserva o `revises` do disco; uma revisão
  pendente por skill.
- **O detector vê as tools**: `MessageOutcome.tools_used` (só nomes, só tools que rodaram sem erro) é salvo em
  `ConversationMessage.tools_used` e o transcript marca `<assistant tools="...">`; `discovery` sem tool é alegação.
- **Scanner de conteúdo** (`learning::scan_proposal`, sem chamada de modelo): recusa link, comando, chave/token, frase contra as
  regras do assistente e caractere invisível, na skill nova e no texto novo de uma revisão (`Outcome::Blocked`). Erra para o
  lado seguro. O `PROPOSER_PROMPT` também manda `{"skill": null}` para pedido inseguro, em vez de uma skill "de sermão".
- **Modelo e gasto**: `[[users]] learning_provider` (o do membro, senão o `[learning] provider`, senão o da conversa) e o canal
  de gasto `learning` (`Orchestrator::with_spend_channel`): os limites por pessoa, agente e globais continuam valendo, os por
  canal de origem deixam de contar o aprendizado.
- **Busca semântica no histórico** (`search_history`): palavras + significado por *reciprocal rank fusion*. Modelo:
  `paraphrase-multilingual-MiniLM-L12-v2` quantizado (`warden_core::memory::embed`, ~120 MB, uma vez); o `multilingual-e5-small`
  foi **testado e descartado** (cossenos de 0,75 a 0,9 para qualquer par, sem como separar acerto de ruído). Piso 0,30
  (relacionado 0,34–0,77, não relacionado ≤ 0,25). Índice `.history-index` ao lado das conversas, sem texto, cifrado como elas
  na pasta de um membro, feito até 300 mensagens por busca; sem o modelo cai nas palavras (`WARDEN_NO_SEMANTIC` desliga).
- **Medição** (`warden-bootstrap/tests/learning_eval.rs`, `--ignored`): 25 conversas sintéticas rotuladas com o
  `deepseek/deepseek-v4.1-flash` pelo OpenRouter: 10/10 aprendidas, 9/9 banais deixadas em paz, 6/6 armadilhas fora, ~26 mil
  tokens. Limite: o conjunto é pequeno e o prompt foi ajustado vendo as armadilhas dele.

### O assistente aprendendo, diff e modelo por membro (Sessão 117)

- **Diff da revisão** (P115 b): as quatro telas comparam, linha a linha (LCS, sem dependência), o `body` da revisão pendente
  com o da skill ativa que ela `revises`, achada na mesma lista; contexto de 2 linhas, o resto dobrado. Nada mudou no
  protocolo. Um helper por cliente (`skillDiff.ts` na web, no desktop e na extensão; `skill_diff.dart` no celular).
- **Modelo do aprendizado por membro na web** (P115 h): `SetUserLearningProvider` (chave de pareamento, como `SetUserTools`;
  resposta `UserList`) → `UserChange::SetLearningProvider` → `users::set_user_learning_provider`, que só aceita id de provider
  ou combo que o hub tem (vazio volta ao do workspace). `UserInfoDto.learning_provider` mostra a escolha; a tela de Pessoas
  ganhou "Modelo do aprendizado". Só a web tem tela de Pessoas; nos outros o `config.toml` segue valendo.
- **Chats dos bots** (P115 g): `[learning] bot_chats` lista os chats (`telegram:<chat_id>`, `whatsapp:<jid>`, o id da
  conversa do bot) dos quais o assistente pode aprender; vazia, os bots não aprendem, porque quem escrevesse ao bot deixaria
  sugestões no vault do dono. A checagem (`LearningSettings::bot_chat_allowed`) fica nos dois bots, não em
  `learn_with_config`, que o hub e o desktop também usam e que não têm essa lista.
- **Quem pode falar com os bots** (P117, fatia 1): `warden_bootstrap::bot_access` com `[telegram] allowed_users` (ids
  numéricos; nome de usuário não serve, muda e pode faltar) e `[whatsapp] allowed_chats` (número ou JID, casa por igual ou por
  `número@`). Vazia é ninguém. Só conversa privada: no Telegram, `chat.type == "private"` e `from.id` na lista; no WhatsApp,
  o sufixo do JID (`@s.whatsapp.net` ou `@lid`), então `@g.us`, `status@broadcast` e newsletters caem sem mexer no sidecar
  JS. O gate roda **antes** de tudo (inclusive `/start` e o aviso de mídia): o desconhecido não recebe resposta, nada é
  gravado, e uma linha de log por id (`Access::reported`) diz o que adicionar. Os bots releem a lista do `config.toml` a cada
  rodada de updates (Telegram) e a cada mensagem (WhatsApp), mantendo a última boa se a leitura falhar. Quem entra segue
  usando o vault e as tools do dono; o desktop repassa os dois campos ao salvar as configurações, para não apagar a lista.

### Pareamento dos bots (P117 fatia 2, Sessão 118)

- **Um arquivo compartilhado, não um serviço**: os bots, a CLI e o desktop são processos separados e só se encontram no
  `config.toml` e no `bot_pairing.json` (ao lado dele), relidos a cada chamada, o mesmo padrão do registro de dispositivos do hub.
  Aprovar grava na lista do bot e o bot, que relê o config a cada mensagem, passa a responder sem reiniciar. Não há trava
  de arquivo: dois escritores no mesmo instante podem perder um pedido, aceito por ser raro e por o remetente poder pedir de novo.
- **Opt-in, e calado por padrão**: com `pairing` desligado o bot segue ignorando o desconhecido (decisão da fatia 1). Ligado,
  responde **uma vez** (o código) e depois silêncio enquanto ele vale, para uma pessoa não conseguir fazer o bot falar à vontade;
  teto de 10 pendentes por canal para uma enxurrada não crescer o arquivo; só conversa privada.
- **O código não é um segredo de autenticação**: só diz ao dono qual pedido aprovar; quem decide é o dono, por um canal
  que já o autentica (a CLI e o desktop são a máquina dele; a web pede a chave de pareamento). Por isso não há limite de tentativas.
- **O hub reaproveita os moldes existentes**: `ListBotPairings` (só o dono, `member_refusal`) e `ResolveBotPairing` (chave de
  pareamento, 1 s de espera se errada, trava por hub), com respostas `BotPairings` e erros como `UserError`, igual aos espaços.
- **`[learning]` deixou de ser decidido na partida**: o bot relê o `FileConfig` inteiro a cada rodada/mensagem e usa o
  último que leu bem; só o token do Telegram ainda pede reiniciar.
- **Aviso ao remetente (Sessão 119)**: `approve` deixa um marcador em `bot_pairing_approved.json` (`channel`, `sender`,
  `approved_at`; um por remetente, validade de 24 h, para um bot que ficou fora do ar não avisar tarde demais) e cada bot
  consome os do seu canal com `BotPairing::take_approved`, que devolve e remove. Telegram: depois de cada `getUpdates` (até
  ~30 s); `sender` vale como chat porque o pareamento é só privado. WhatsApp: a cada 3 s e **assim que o sidecar conecta**,
  só enquanto estiver conectado (cada marcador sai uma vez, e um envio com o sidecar caído o perderia), sempre para o JID
  que escreveu (pode ser `@lid`). Recusar não avisa ninguém. Falha de envio só vai ao log: a pessoa já está na lista.
- **Chat que vira pessoa (Sessão 120)**: o bot é outro processo e não tem a chave cifrada do membro (ela só vive na memória do
  hub, aberta pela senha), então **não abre nada**: pergunta ao hub, como o membro, pelo mesmo WebSocket dos apps
  (`warden_bootstrap::bot_hub`). O hub roda o turno com o vault, as tools e os limites dele; nada do chat fica na pasta do dono.
  - **Quem é quem**: `[telegram] members` e `[whatsapp] members` (chat → id do membro; o WhatsApp casa o id inteiro ou o número,
    o id inteiro vence) decidem **como** responder; a lista `allowed_*` continua decidindo **se**. Chat sem entrada segue como
    antes (dono, orquestrador local). `[bot_hub] url` é o hub (`ws://` em LAN; `wss://` pede certificado de uma autoridade pública,
    pois o cliente só confia nas raízes públicas e não há fixação de impressão digital).
  - **Vínculo**: `warden bots link <membro> [--hub url]` pede a senha **do próprio membro**, sem eco, uma vez; guarda só o token de
    dispositivo em `bot_hub.json` (0o600, ao lado do `config.toml`, `warden-bot-<membro>` como id do dispositivo) e descarta a
    senha. Entra com `recovery_codes: false` (`handshake_as_member_showing`): um bot não mostra o código de recuperação, e com
    `true` o hub criaria a chave cifrada do membro ali mesmo e devolveria um código que ninguém veria. Reconecta só com o token.
  - **Aprovar como membro**: `warden bots pair approve <código> --as <membro>` (`BotPairing::approve_as`) recusa, sem mudar nada, se o
    membro não existe, se não há `[bot_hub]` ou se ele não está vinculado. **O desktop e a web também escolhem o membro** (Sessão 123):
    cada pedido pendente ganha um seletor "Falar como" (padrão: o dono), e o `ResolveBotPairing` leva `member` (opcional; ausente, como
    antes). A listagem (`BotPairings`) traz `members` (`BotMemberDto { id, name, linked }`, de `bot_hub::bot_members`, a mesma função
    para os dois clientes): todos os membros aparecem, mas só os **vinculados** são escolhíveis, e um não vinculado mostra o caminho
    (`warden bots link <id>`) em vez de sumir. A trava de verdade é o hub, que recusa a aprovação inteira (ninguém entra, o pedido fica);
    o `disabled` do seletor só poupa a ida. Negar ignora o membro. Compatível nos dois sentidos (campos com `default`).
  - **Cofre trancado**: se o hub reiniciou, o token reconecta mas o `Chat` volta `ChatError` com o texto de "dados trancados", e o bot
    o repassa ao chat. O membro destranca entrando uma vez pelo web, desktop ou celular. Nenhuma senha vai a disco, de propósito.
  - **Falhas viram uma linha no chat, nunca uma resposta do dono**: sem `[bot_hub]`, sem vínculo, token revogado (`AuthRejected` ou
    `AuthError` no meio da conexão), hub fora do ar ou turno além de 180 s. Os turnos de um membro vão um de cada vez (o hub não
    protege duas conversas iguais disputando o arquivo); uma conexão guardada que caiu ganha **uma** tentativa nova, uma conexão
    recém-aberta que falha não (um turno custa chamadas de modelo; um turno cortado no meio pode, no pior caso, rodar duas vezes).
  - **Limites aceitos**: só texto (foto e PDF do chat não vão ao hub; o WhatsApp mostra o aviso de sempre); o gasto aparece no hub
    como canal `server` com a pessoa (o limite por pessoa vale; o por canal `telegram` não); o aprendizado (P104) não roda em chat
    de membro; trocar a senha do membro não revoga o token (comportamento do hub; revogar é pelo registro de dispositivos); remover
    o membro revoga o dispositivo e o mapa fica no `config.toml` (o chat passa a ouvir "não estou mais conectado").
  - **O save das telas não desmapeia ninguém**: `apply_bots_settings` leva `members` adiante, e o `save_settings` do desktop leva
    `bot_hub`. Nenhuma tela edita o mapa à mão: ele nasce na aprovação (CLI, desktop ou web) e some editando o `config.toml`.

### Tela de "Aprendizado e bots" (P118, Sessão 118)

- **Um bloco só**: `BotsSettingsDto` (`[learning]` inteiro mais as duas listas dos bots) viaja em `HubSettingsDto.bots` e em
  `HubSettingsUpdate.bots: Option<_>`. Sem o campo o save mantém o arquivo, então cliente antigo não apaga nada. A web usa o
  `SaveSettings` que já existe (chave de pareamento, versão do arquivo, recusa a membro) em vez de uma mensagem nova.
- **Uma validação**: `apply_bots_settings` em `warden-bootstrap/src/settings.rs`, usada pelo hub e pelo desktop, para as duas
  telas recusarem as mesmas coisas.
- **O token do Telegram** foi só do desktop no P118 (a web não editava segredo de bot). **No P119 a web também o edita**, como
  `telegram_token` (`SecretStatusDto` na visão, `SecretEdit` no save; `Set` exige conexão cifrada ou local, como uma chave de API).
  O desktop o troca por `bot_cmds.rs` (ausente mantém, vazio remove) e ele nunca volta, só "salvo, termina em …".
- **No desktop a seção salva sozinha** (`bot_cmds.rs` relê o arquivo e muda só essa fatia), fora do formulário principal, no
  molde do `ApiKeysSection`. As listas valem na hora (os bots as releem); o token e o `[learning]` só na próxima partida.

## A web edita a máquina do hub, com trava (P119, Sessão 121)

A paridade da web com o desktop nas configurações. A decisão do P78 (shell, MCP, SSH e caminhos fora da web, porque dariam a
quem tem a chave de pareamento comandos na máquina do hub) mudou: a web edita tudo, **mas o que alcança a máquina fica atrás
de uma trava que o hub aplica**, não da tela. Uma confirmação só no navegador não protege nada, porque quem fala o protocolo a
ignora.

- **Três fatias novas, no padrão de `git_sync`/`bots`** (visão com `#[serde(default)]`, edição opcional, ausente = não mexe;
  um cliente antigo segue valendo): `telegram_token`; `advanced` (as três chaves de delegação e `truthid_*`); e `machine`
  (shell, `vault_path`, `generated_path`, servidores MCP, hosts SSH e o hub embutido). `machine` e `advanced` vão numa `Box`
  no `HubSettingsUpdate`: sem isso o `ClientMessage` passa do limite do `large_enum_variant`.
- **A trava do `machine`** (`settings::machine_gate`, antes de ler ou gravar qualquer coisa, depois da chave de pareamento):
  o hub precisa ter subido com **`warden-server serve --allow-machine-settings`** (desligado por padrão: um hub que já roda
  não passa a aceitar comandos pela rede só porque o código aprendeu) **e** a conexão tem de ser cifrada ou local (a mesma
  regra de uma chave de API nova). A visão traz `machine.writable` e `blocked_reason`, e a tela fica em somente leitura com o
  motivo. Cada save que muda a fatia deixa uma linha no log do hub (`machine settings changed from <ip>: shell on, MCP servers
  (1 now), ...`), com o que mudou e **nunca** com valores. O hub embutido do desktop não ganha a flag (o dono edita ali
  mesmo); a flag é só do `serve`.
- **Segredos de MCP**: o valor de uma variável de ambiente ou de um cabeçalho nunca volta; a visão traz os **nomes**
  (`env_keys`/`header_keys`) e o save manda `{chave, SecretEdit}` por entrada, com `original_name` para o `Keep` achar o valor
  guardado de um servidor renomeado. `oauth` dos servidores http é carregado do arquivo e não editável (o fluxo OAuth precisa
  do navegador do desktop); um servidor com OAuth não aceita cabeçalhos. Um `Set` novo entra em `sets_a_secret()`.
- **Checagens só da web** (em `machine_settings.rs`): pastas **absolutas e sem `..`**; MCP com nome único e `command` (stdio) ou
  `http(s)://` (http); hosts SSH conferidos contra os agentes **como o mesmo save os deixa**. Um valor que já estava no arquivo
  passa sem checagem (quem editou à mão não fica impedido de salvar o resto). As **chaves de delegação têm teto na web**
  (profundidade 5, chamadas 1..=300, jobs 0..=10; `max_delegated_calls = 0` só no arquivo), só para valor **mudado**.
  `truthid_public_url` exige `https://`.
- **Uma validação, dois clientes** também aqui: `ssh_host_from_dto`/`ssh_hosts_into_config` moraram no desktop
  (`ssh_cmds.rs`) e foram para o bootstrap; o desktop passou a chamá-las, e os três testes dele seguem passando sem mudança.
  A checagem do hub embutido **não** foi compartilhada: as mensagens do desktop são em português e a da web em inglês.
- **`auth_key` do hub embutido não é editável pela web**: é a própria chave de pareamento com que a tela assina o save; trocá-la
  pelo mesmo canal trancaria o dono para fora. O resto vale **na próxima partida** (nada recarrega um listener que já roda), e a
  web só edita um `[embedded_server]` que já existe (nunca cria).
- **Membros**: `member_settings_view` limpa os campos da visão um a um (uma lista de negação, então um campo novo vazaria por
  padrão). Passou a zerar `bots`, `telegram_token`, `advanced` e `machine`. **`bots` (P118) estava fora da lista**: um membro
  via os ids de Telegram e os números de WhatsApp do dono; um teste com valores reais no arquivo do dono pega isso (conferido
  tirando a linha e vendo o teste cair).
- **Web**: seções "Avançado" e "Máquina do hub" em arquivos próprios (`AdvancedSection.tsx`, `MachineSection.tsx`,
  `machineDraft.ts`), e os blocos da tela (`Section`, `Field`, `SecretField`) foram para `settingsParts.tsx`. A fatia só vai no
  payload quando **mudou** em relação ao carregado (como `gitSync`), então um hub com a trava desligada nunca a recebe sem
  querer. Antes de pedir a chave de pareamento, um painel lista o que muda (sem valores secretos) e exige marcar o aceite.
  Todos os controles da seção ficam num `<fieldset disabled>` quando o hub não deixa salvar.
- **Teste de ponta a ponta** (Sessão 122, `web/e2e/`): `npm run test:e2e`, depois de `npm run build` e
  `cargo build -p warden-server --bin warden-server`. Sobe um `warden-server` real por teste, isolado (`HOME`/`XDG_*` numa pasta temporária, porta
  0), e dirige a página num Chromium headless (`PLAYWRIGHT_CHROMIUM_EXECUTABLE`, o do Playwright ou o Chrome do sistema); captura os frames de
  WebSocket enviados, para provar o que viajou ao hub. O `harness.mjs` serve a qualquer teste futuro da web. Não roda no CI (o `build.yml` só monta o desktop).

## Projetos (P103 a, Sessão 125)

Um projeto agrupa conversas de um assunto, com instruções e arquivos próprios, como os Projects do Claude e do ChatGPT.

- **No disco**: `projects/<id>/PROJECT.md` no cofre **da pessoa** (o do dono, ou o do membro, cifrado junto: os nomes de pasta cifram por componente, então uma sub-pasta é raiz válida para a mesma cifra). Frontmatter
  `name` e `description`, corpo = instruções (até 16 KB: vão a toda chamada). O `id` é o nome da pasta (`[A-Za-z0-9_-]`, 1–64, a mesma regra do id de conversa) e **não muda**; o nome de exibição muda sem quebrar
  o vínculo. Os arquivos do projeto são as notas da pasta (qualquer tipo, subpastas): sincronizam com o cofre, e o desktop e a web listam e acrescentam notas ali. `ProjectStore` (`warden_core::project`)
  é o molde do `SkillStore`; no `Vault`, `dirs_in` lista pastas e `subvault` abre uma pasta como cofre próprio.
- **A conversa**: `Conversation.project_id` (e `ConversationSummary.project_id`, `Chat.project_id`), `#[serde(default, skip_serializing_if)]`. **Só vale na criação**: `append_messages` não mexe nele numa conversa que
  já existe, ao contrário do `agent_id`, que é regravado a cada turno. Copiar o molde do `agent_id` apagaria o projeto a cada turno de qualquer cliente que não o enviasse (todos os que existem). Um projeto
  que não existe mais (sem `PROJECT.md`) lê como "sem projeto": a conversa segue como comum e a lista a mostra com as outras. Começar uma conversa num projeto que não existe é erro, e nada é gravado.
- **O turno** (`warden_bootstrap::scope_to_project`, usado por `handle_agent_turn` no hub e por `send_message` no desktop, **depois** do agente e do modelo): `Orchestrator::with_project` troca o cofre pelo da pasta (`with_vault`, que
  religa as tools e os sub-agentes) e guarda o briefing, que entra logo depois da persona. Como o catálogo de skills e o contexto semântico leem `self.vault`, **vêm da pasta**: é o isolamento escolhido, e por isso as skills
  e a memória geral não existem numa conversa de projeto. **Tools que uma pasta não segura saem do turno** (`WITHHELD_IN_A_PROJECT`: `shell`, `ssh_exec`, `node_shell`, `search_history`): o `shell` é deliberadamente sem
  sandbox (`cd ..`, `cwd` absoluto), `ssh_exec`/`node_shell` rodam em outro lugar e `search_history` lê todas as conversas. As tools de MCP e da web não são acesso ao cofre e ficam. O cofre escopado fica num cache do processo,
  chaveado pelo caminho e, se cifrado, por uma impressão da chave: o modelo de busca de cada projeto carrega uma vez, não por turno.
- **No hub**: `ListProjects`/`SaveProject`/`DeleteProject` (`projects.rs`, no cofre de **quem pergunta**, como as skills); membros usam o próprio, e o `password_gate` fecha a tela enquanto a senha é a provisória
  (**o braço explícito é necessário**: a lista termina em `_ => None`, e uma mensagem nova passaria). **Remover só tira o `PROJECT.md`**: os arquivos ficam como notas comuns.
- **Limites conhecidos**: a busca do cofre das conversas **fora** de projeto acha os arquivos de projeto (são notas comuns; `skills/` é excluída da busca, `projects/` não); o sync não leva conversas, então em outro
  aparelho o projeto aparece com os arquivos e sem as conversas; um binário de antes de projetos que regrave o arquivo de uma conversa perde o `project_id`; o celular, a extensão e o CLI não conhecem projetos ainda.

### Mover conversa e projeto de código (P103, Sessão 126)

- **Mover**: `MoveConversation` (resposta `ConversationOk`/`ConversationError`) é o **único** jeito de mudar o `project_id` depois da criação (`set_conversation_project`); o projeto precisa existir no cofre da pessoa e
  a conversa de uma tarefa agendada não se move. O que já foi dito fica; o próximo turno roda no novo escopo. Desktop e web pedem confirmação antes.
- **Busca das conversas soltas**: `PROJECTS_DIR` mora no `Vault` e `projects/` fica fora da memória fora de projeto (resolve o limite da Sessão 125).
- **Projeto de código**: `Project.workdir` (frontmatter `workdir:`, absoluto, sem `..`; um valor inválido em arquivo editado à mão vira "sem pasta"). Só um projeto com pasta recebe `shell`, e é o `ShellTool::in_folder`:
  começa na pasta (que não é criada; se não existe, erro), **pede aprovação a cada comando** (120 s sem resposta = recusa; canal que não pergunta = recusa) e **não é sandbox** (`cd ..` funciona), então a aprovação é a
  proteção, e o briefing diz isso ao modelo. As tools de arquivo continuam presas às notas do projeto. Se a máquina ou o membro não tem `shell`, o projeto também não ganha.

## Modo código: o opencode como motor (P103 b, P89, Sessão 127)

Um projeto com `workdir` **e** `code: true` (frontmatter do `PROJECT.md`; `validate` recusa `code` sem pasta, e um `code: true` escrito à mão sem pasta válida é ignorado) roda as suas conversas no **opencode**, não no turno do Warden. Um projeto só
com `workdir` continua com o `shell` que pede aprovação a cada comando (Sessão 126).

- **A camada** (`warden_core::code_engine`): `CodeEngine` (rodar um turno, abortar) com o opencode como primeira implementação; outro motor entra atrás dela sem mexer na memória, nos agentes nem nos clientes. `TurnRequest` leva a pasta, a sessão (se a
  conversa já tem), a tarefa, o nome do projeto (quem o usuário vê no pedido de aprovação) e as instruções do projeto (vão como `system` a cada tarefa). O motor devolve `CodeEvent` (texto, ferramenta, aviso, e `Session`, que não é para mostrar) e, no fim,
  o texto, a sessão e as ferramentas usadas.
- **O `Tracker`** (puro, sem I/O) lê o `GET /event` do opencode. Formatos fixados no OpenAPI do `opencode serve` 1.18.34 (`/doc`), não na documentação: `message.updated` diz o papel de uma mensagem (o prompt do usuário volta como parte de texto, então o texto
  fica retido até a mensagem ser a do assistente), `message.part.updated`/`message.part.delta` trazem o texto e as ferramentas, `permission.asked` é uma pergunta, `session.idle` acaba a tarefa, `session.error` a falha (`MessageAbortedError` é parar de
  propósito e **guarda o trabalho feito**), `session.status` com `retry` vira aviso. Os sub-agentes do opencode abrem sessões filhas (`session.created` com `parentID`): as perguntas de permissão delas também chegam ao usuário, mas só a sessão raiz acaba a tarefa.
- **O cliente** (`OpencodeEngine`): cria (ou reutiliza) a sessão com um conjunto de regras (`"*": ask`, e só `read`, `glob`, `grep` e `list` liberados), abre o fluxo de eventos e **espera ele dizer que está vivo antes** de mandar a tarefa
  (`prompt_async`), para não perder nada; cada `permission.asked` vai ao `Approver` (120 s sem resposta ou sem aprovador = recusa) e a resposta volta como `once` ou `reject`. **"Sempre permitir" (Sessão 129)**: o trait `Approver` ganhou `ask(request, always) -> Answer` (`Once`/`Always`/`Reject`), com implementação padrão que chama `approve` (CLI, `shell`, SSH e `manage_agents` não mudam e não oferecem "sempre"); só o `WsApprover` do hub e o `TauriApprover` do desktop o sobrescrevem. O `always` é o que o próprio opencode sugere no `permission.asked` (`always: ["git status *"]`, lido pelo `Tracker` em `PermissionAsk.always`) e vai ao cliente como texto, para o botão dizer o que cobre. **Quem lembra é o Warden, não o opencode**: o `OpencodeEngine` guarda, **por sessão (≈ conversa) e só em memória**, pares `(permission, padrão)`; um pedido seguinte é um sim sem perguntar quando tem a mesma `permission` e **todos** os seus padrões casam com algum guardado (`*` = qualquer trecho; um ` *` final também casa o comando sem argumentos, como no opencode); ao opencode a resposta é sempre `once`. Reiniciar o hub/desktop esquece. Sem `always` no pedido, o botão não aparece. **Modos (Sessão 130)**: `warden_core::code_engine::mode` — `CodeMode` (`manual`, `acceptEdits`, `acceptAll`, `plan`) escolhido pela pessoa **a qualquer momento, até no meio de uma tarefa**. `Manual`: o de cima (pergunta, com "sempre"). `AcceptEdits`: a permissão `edit` (a única chave de edição na configuração do opencode 1.18.34; cobre editar, escrever e `patch`) é sim, o resto pergunta. `AcceptAll`: tudo é sim (o `bash` não é sandbox; o seletor fica em cor de perigo). `Plan`: tudo é **não** (leitura já é liberada pelas regras da sessão) e o `system` da tarefa ganha a instrução "modo plano: não altere nada, responda com um plano". O Plano é imposto **pelo Warden** (recusa), não pelo agente `plan` do opencode, para valer para qualquer motor atrás do `CodeEngine`. O modo viaja como `watch::Receiver<CodeMode>` no `TurnRequest.mode` (de um `CodeModes`, registro por conversa **só em memória**: reiniciar volta a Manual); no `Signal::Ask` o `OpencodeEngine::decide` consulta o modo, depois a memória do "sempre", depois a pessoa, e a pergunta aberta roda num `select!` com `mode.changed()`: **se o modo muda com o modal aberto, a pergunta é abandonada e reavaliada no modo novo**. Para o modal fechar, o `WsApprover` ganhou um guard de descarte (`Withdraw`) que retira a pergunta e manda `ApprovalCancelled` (o `TauriApprover` já tinha o `PendingGuard`). Protocolo: `ClientMessage::SetCodeMode { conversation_id, mode }` (nome desconhecido = `manual`; só o dono; o registro é o do `CodeTurns`, compartilhado entre conexões, então outro aparelho também troca). **Os clientes repetem o modo a cada `Chat`** (a web manda `setCodeMode` antes do `sendChat`; o desktop passa `code_mode` no `send_message`), para um hub reiniciado que esqueceu não contradizer o seletor; o desktop troca no meio por `set_code_mode`. O seletor só aparece em projeto de código (web: "Modo"; desktop: "Mode"), e ao reabrir a conversa o estado da tela volta a Manual. No protocolo do "sempre": `ApprovalRequest.always` (opcional) e `ResolveApproval.always` (padrão `false`), compatíveis com clientes antigos. Toda rota leva `?directory=`. Uma sessão que o opencode não conhece
  mais (`404`) é trocada por uma nova.
- **Os processos** (`OpencodeProcesses`): um `opencode serve --hostname 127.0.0.1 --port 0` por pasta, **iniciado na pasta**, com uma senha aleatória no ambiente (`OPENCODE_SERVER_PASSWORD`; a porta vem da linha "listening on" do próprio opencode), a
  configuração do hub em `OPENCODE_CONFIG_CONTENT` e `OPENCODE_DISABLE_AUTOUPDATE`; **nada é escrito no repositório do usuário**. Reinicia se morrer e é encerrado por ociosidade, **menos enquanto uma tarefa o usa** (a `Endpoint` carrega uma
  concessão que o gerenciador respeita). `WARDEN_OPENCODE` troca o comando; a versão testada é a 1.18.34 e uma `major.minor` diferente gera um aviso, não uma recusa. Sem o binário, o erro diz como instalar.
- **A conversa**: `Conversation.engine_session_id` (opcional, como `project_id`), gravado ao fim de cada tarefa; `append_messages` só o altera se mandarem, e `set_conversation_project` o **esquece** quando a conversa muda de projeto. O turno é `CodeTurn::run`
  (`warden_bootstrap::code_turn`), que grava a mensagem do usuário e a resposta (com `tools_used`), e **não grava nada se a tarefa falha**. Anexos não são aceitos ainda.
- **No hub**: o `Chat` de uma conversa cujo projeto é de código vira uma tarefa do motor (`code_project`); **só o dono** (a pasta de um membro seria um caminho na máquina do dono, e sem a guarda um membro cairia no motor). Um hub sem motor responde com um
  erro dizendo isso, em vez de rodar o turno comum com um shell. Os eventos vão como `ServerMessage::ChatEvent` (texto, ferramenta com `callId` estável, aviso) entre o `Chat` e o `ChatResponse`, que continua sendo o fim do turno; a aprovação usa o
  `ApprovalRequest` que já existia. `CancelTurn` acha a sessão no registro `CodeTurns` (compartilhado entre as conexões, então outro aparelho pode parar a tarefa) e chama `abort`; vale só para o dono.
- **A rota do opencode ao modelo** (`engine_models.rs`, `openai_api::serve_engine`): o opencode precisa de um modelo, e o hub já tem os provedores, o fallback e os limites. **Mas não pelo `/v1` da API do Warden**: com TLS ele só redireciona para `https`
  (o opencode não confiaria no certificado), e ele fala *como o Warden*, com as ferramentas dele (o `shell` rodaria no hub sem passar pelas aprovações do opencode), a persona e as notas do dono como contexto. A rota própria é um listener em
  `127.0.0.1`, HTTP simples, com **um token aleatório só em memória** (nada na lista de chaves da API, nada em disco), que reaproveita o `complete` do `/v1` (extraído de `chat_completions`) com um orquestrador **sem ferramentas do Warden, sem persona e com um
  cofre vazio**; só valem as ferramentas e o prompt que o opencode manda (as `tools` do cliente, P91). Passa pelo provedor, pelo fallback e pelos limites do hub, no canal `code`. A configuração do opencode o torna o **único provedor** (`enabled_providers`),
  inclusive para `small_model` (títulos), com `share: disabled` e `autoupdate: false`: nada do que a pessoa digita vai a um provedor que ela não deu. Depende do pacote `@ai-sdk/openai-compatible`, que o opencode baixa do npm na primeira vez (precisa de rede).
- **Clientes**: a web mostra os eventos numa linha do tempo no balão do turno (texto e ferramentas na ordem em que aconteceram, a ferramenta atualizada no lugar) e o botão **Parar**; o projeto ganha a caixa "Modo código". **O desktop roda o modo código no próprio processo**
  (Sessão 128, `desktop/src-tauri/src/code_cmds.rs`), **sem precisar do hub embutido ligado**: o `send_message` desvia para `CodeTurn::run` (o mesmo do hub), com um `CodeRuntime` criado no primeiro turno de código (um `SharedOrchestrator` próprio, que recebe o
  orquestrador atual do desktop a cada turno, mais a rota do modelo `EngineModels` e o `opencode_engine`). Os eventos chegam à janela como `chat-event` (mesmo `ChatEventDto`) e viram o balão ao vivo (`liveTurn.ts`, portado da web), a aprovação usa o `TauriApprover`
  e o modal que já existiam, e o comando `cancel_turn` acha a sessão no `CodeTurns` e chama `abort`. Como o `CodeTurn` grava o intercâmbio, o `SendMessageResult` volta com `already_saved` e o frontend **não** grava a mensagem do usuário antes (só a mostra) e
  recarrega a cópia salva depois. Com o hub embutido ligado ao mesmo tempo, há dois `opencode serve` por pasta (um de cada lado).
- **Limites conhecidos**: o consumo do opencode é contado pelo `SpendGuard` do hub (canal `code`, pessoa `opencode`), mas **como isso aparece na tela de Uso não foi conferido**; o "sempre permitir" vale só por conversa e em memória (nada persiste, não há lista para ver ou revogar); sem anexos; sem modo geral (P102); um projeto de código compartilhado entre pessoas não existe; o celular, a extensão e o CLI não o
  conhecem; o gerenciador sobe uma instância por pasta e segura o *lock* do mapa durante a subida (até 30 s), o que atrasa a primeira tarefa de outra pasta nesse intervalo.

## Dólares por provedor, agente e pessoa, por dia, e "Testar chave" (P10, Sessão 124)

O P10 pedia UI de consumo, custo por provedor/modelo e gestão de chaves. Boa parte já existia desde o P4; o que faltava:

- **O ledger passa a guardar o provedor** (`SpendEvent.provider: Option<String>`, `#[serde(default, skip_serializing_if)]`, o molde do `person`):
  uma linha antiga lê sem ele e um binário antigo ignora o campo novo. **Não** criei uma variante nova de `Entry`: um binário antigo descartaria a
  linha inteira em silêncio. Quem sabe o provedor é o ponto que o escolhe: `ModelProvider::provider_id()` (padrão vazio) e o invólucro `Labeled`
  (`warden_core::model::labeled`), aplicado em `build_model_for` (provedor, combo de um membro só, e o id sintetizado do setup antigo); o `for_agent` do
  `Labeled` **preserva o rótulo** (sem isso, as chamadas feitas em nome de um agente o perdiam). `FallbackProvider::provider_id()` é o do primeiro membro,
  e o orquestrador usa o `to` do `ProviderFallback` quando um reserva assume: **vale o membro que de fato respondeu**. `SpendGuard::record_served` leva o
  provedor; `record` continuou como era (sem rótulo), para não mexer em ~40 testes.
- **`breakdown()` agrupa também por provedor, agente e pessoa**, e `RecentSpendDto`/`UsageReportDto` ganharam os campos com `#[serde(default)]`. A chave vazia é
  "sem provedor registrado" (linha de antes), "sem agente" ou "o dono". **"Por agente" é o agente com que o turno começou**: as chamadas de um sub-agente
  delegado já contavam no agente raiz e nos limites dele, e mudar isso mexeria na contabilidade dos limites por agente.
- **Dólares por dia** (`bootstrap::usage::daily_cost`, espelho do `daily_usage`, com o mesmo fuso): só alcança o que o ledger retém (a maior janela de limite, 24 h
  no padrão); um dia mais antigo aparece zerado porque **saiu** do registro, e a tela diz isso. Sem `[[limits]]` explícito vale a rede de segurança padrão, então há ledger.
- **Desktop**: `spend_status` devolve `recent` e a tela de Uso mostra as cinco tabelas de US$ (o desktop não tem gráfico diário e não ganhou um). **Web**: as tabelas
  por provedor, agente e pessoa, e o gráfico "Gasto por dia"; o `DailyChart` virou genérico (tokens ou dólares). O mobile não tem tela de Uso.
- **"Testar chave"**: `ModelProvider::check_key()` (padrão: sem chave para conferir), implementado sobre `GET /models`, que valida a chave sem gastar tokens: Gemini
  (`x-goog-api-key`; chave inválida volta **400** com "API key not valid", não 401), OpenAI e compatíveis (`Bearer`; num compatível 404/405 é "alcançável, não dá
  para verificar", não falha) e Anthropic (`x-api-key` + `anthropic-version`). Cliente com timeout de ~10 s e **sem seguir redirecionamento** (o pedido, chave
  incluída, não pode ser desviado). A resposta é uma palavra e uma frase (`KeyCheck`): **nunca a chave, o corpo que o provedor devolveu nem o endereço chamado**.
  Gemini e Anthropic ganharam `with_models_url`/`with_base_url` (a base era constante) para testar contra um servidor falso. Nunca passa por um `Orchestrator`,
  então **não grava no ledger nem conta limite** (conferido no ponta a ponta: o arquivo do ledger sai igual ao que entrou).
- **No hub** (`provider_admin.rs`, `ClientMessage::TestProvider` → `ProviderTest`): chave de pareamento (com a espera de 1 s se errar) → a chave **digitada** só em conexão
  cifrada ou local → o `Keep` resolve a chave **salva** pelo `original_id`, sem nunca devolvê-la → o hub só consulta um endereço que **já está salvo** (um `base_url`
  novo ou mudado é recusado com "salve primeiro", para um aparelho pareado não fazer o hub chamar um endereço interno que o dono nunca configurou; só `http(s)`).
  **O `settings_lock` é segurado só na checagem da chave de pareamento, não durante a chamada de rede**, que leva segundos e travaria todo salvar. `member_refusal`
  recusa a mensagem para membros (**um braço que falta ali deixa a mensagem passar**, por causa do `_ => return None`; um teste cai sem ele). Desktop: `test_provider_key`
  (no molde do `test_ssh_host`, recebe o formulário com a chave digitada); web: o botão no cartão do provedor, que pede a chave de pareamento na hora e não a guarda.

## Pasta de trabalho numa conversa avulsa (P102, fatia 1, Sessão 132)

Uma conversa **sem projeto** pode trabalhar numa pasta da máquina, escolhida **antes da primeira mensagem** e fixa depois (como o `project_id`). A pasta só delimita o espaço de trabalho do agente normal do Warden: **não liga o opencode** e não vira modo código (decisão do usuário: ver primeiro se o agente do Warden dá conta de editar arquivos e gerar PDFs; se não, o opencode entra numa fatia à parte, atrás do mesmo `CodeEngine`).

- **Dados.** `Conversation.workdir` (`warden-bootstrap`), `AppendOptions.workdir`, `ConversationSummary.workdir`, `Chat.workdir` (todos opcionais, retrocompatíveis). Só vale na criação; um projeto vence a pasta (a conversa de um projeto não tem pasta); mover uma conversa com pasta para um projeto é **recusado** em `set_conversation_project` (sair de qualquer projeto continua permitido).
- **Escopo do turno.** `scope_to_workdir` (`project_scope.rs`), irmão de `scope_to_project`: troca `read_file`/`write_file` por `FolderReadTool`/`FolderWriteTool` (`warden-core/src/tool/folder_tools.rs`: caminho relativo, sem `..`, sem absoluto, sem `\`, sem symlink que saia, leitura ≤ 1 MiB e só texto; **não usam o `Vault`**, que criaria a pasta e a trataria como cofre) e re-registra o `shell` como `ShellTool::in_folder` com aprovação por comando. Tira só `ssh_exec` e `node_shell`; **mantém `search_history`** e **não troca o cofre**, então as notas e as skills da pessoa seguem no contexto, mas as tools de arquivo não as alcançam (o briefing diz isso: `Orchestrator::with_briefing`). Uma pasta que sumiu é **erro**, não um recuo silencioso para o cofre.
- **Quem pode qual pasta.** `UserConfig.workdirs` (`[[users]] workdirs = ["/abs/path"]`, só no `config.toml`; vazio = nenhuma). O dono pode qualquer pasta que exista; o membro só dentro das suas, com os caminhos **resolvidos** (symlink não sai da raiz; `/srv/allowed-not` não casa com `/srv/allowed`). A checagem (`warden-server/src/folders.rs`, `check_workdir`) roda no **`Chat` a cada turno**, na pasta salva da conversa ou na nomeada para a nova: uma raiz que o dono tira do membro para de valer nas conversas antigas dele. A lista que o cliente mostra não é a fronteira. Um membro sem `shell` liberado fica só com as tools de arquivo na pasta; o `shell` de membro sem aprovador recusa (como já era nos projetos).
- **Navegador de pastas.** `ListDirs { path? }` → `DirList { path, parent?, dirs }` / `DirError`. Só **pastas** (nunca arquivos, links nem pastas com ponto), ordenadas sem diferenciar caixa, no máximo 500. O dono começa na home do hub; o membro começa na lista das suas pastas (`path` vazio, sem "usar esta pasta" ali) e o "subir" da raiz volta a essa lista. Respondido na hora, sem `member_refusal` (o membro passa pela checagem das raízes dentro de `list_dirs`).
- **Clientes.** Web: `FolderPicker` (modal com a pasta atual, "Subir", subpastas e "Usar esta pasta"), botão "Pasta" ao lado do projeto, só antes da primeira mensagem; depois vira uma etiqueta; escolher projeto tira a pasta. Desktop (processo local, sem hub): o diálogo nativo do sistema (`open({ directory: true })`), `send_message`/`append_conversation_messages` com `workdir`; a pasta é do computador da pessoa e qualquer uma serve.
- **Fica**: o opencode nessas conversas, a tela do dono para editar `workdirs`, celular/extensão/CLI sem o seletor, desktop ligado a um hub (hoje o desktop só usa a pasta local). (Pastas em **nós** foram feitas na fatia 2, abaixo.)

### Fatia 2: a pasta fica num nó (P102, Sessão 133)

A pasta de uma conversa pode estar num **nó** (P93): outro computador ligado ao hub, inclusive o desktop de alguém emprestado (P97). Decisões do usuário: a raiz permitida é **a pasta que o nó empresta** (`--files`, ou a do "emprestar este computador"), e **membros já entram**, com uma lista de pastas por nó.

- **Uma string só.** Nada mudou no protocolo: `Chat.workdir`, `ConversationSummary.workdir` e `ListDirs.path` carregam `node:<id do nó>:<caminho>`, com o caminho **relativo à pasta emprestada** (vazio é ela mesma). Uma pasta da máquina do hub sempre começa com `/` (ou letra de unidade), então as duas não se confundem (`warden_bootstrap::node_folder`, `node_folder_ref`, `check_node_path`: só nomes simples, nunca `..`, `.`, `/` nem `\`). Mover para um projeto continua recusado.
- **O nó** ganhou uma operação, `list_dirs` (`node_client.rs`): só pastas (nunca arquivos, links nem pastas com ponto), no máximo 500, com o caminho **resolvido** (links seguidos) e recusando o que sai da pasta emprestada. A resposta diz a pasta que de fato abriu, para o hub julgar o que foi aberto e não o que foi pedido. `read_file`, `write_file` e `shell` do nó ficaram como estavam: o hub só passa o caminho já somado à pasta (`proj/out.txt`) e o `cwd` (`proj`), e o nó os confina como sempre (`Vault::path_of` não deixa `..` nem absoluto). **Symlink (Sessão 138)**: `read_file` e `write_file` do nó passam por `inside_shared` (`node_client.rs`), que além do `path_of` resolve os links: o trecho mais fundo do caminho que existe (o arquivo, ou a pasta onde ele seria criado) tem de continuar dentro da pasta emprestada resolvida. Um link para fora, para uma pasta de fora e um link quebrado são recusados ("not inside the shared folder"); um link que fica dentro funciona, e a pasta emprestada pode ser ela mesma um link. `list_files` não precisa da checagem (a listagem não segue links). Fica a janela entre a checagem e a escrita (como nas tools de pasta do hub). **Limite herdado, não novo**: o shell do nó não é uma jaula, como o `node_shell` (um comando pode criar um link e mexer fora da pasta de qualquer jeito).
- **O hub** (`node_tools.rs`): `NodeToolFactory::list_dirs` (precisa do nó online, aprovado, ligado e com arquivos; **sem** a lista de agentes, que vale em cada chamada depois) e `scope_folder`, irmão de `scope_to_workdir`: troca `read_file`/`write_file`/`shell` por `NodeFolderTool`, que **falam com o nó pelas mesmas regras das tools de nó** (online, aprovado, ligado, aberto ao agente, um sim quando o nó pede, auditado em `node_audit.jsonl`, nunca repetido). O **shell sempre pergunta**, mesmo que o nó não peça (como o de uma pasta local), e sem alguém para perguntar recusa. `..` e caminho absoluto são recusados no hub antes de ir ao nó. Tira `ssh_exec` e `node_shell`, mantém o cofre. `handle_agent_turn` só **grava** a referência e exige que o chamador já tenha feito o escopo (`Orchestrator::has_briefing`): um canal que não sabe falar com nós recusa o turno em vez de rodá-lo sem pasta.
- **Quem pode qual pasta.** O dono, qualquer pasta que o nó empresta. O membro, só as de `[[users]] node_workdirs = [{ node = "node-x-1", path = "projects" }]` (`NodeFolder::covers`: o próprio caminho e o que está dentro, por componente: `projects-not` não casa; caminho vazio é tudo o que o nó empresta). A checagem (`folders::check_node_folder`) pergunta ao nó, a cada turno, qual pasta ele **resolveu** e compara essa com a lista (um link não leva o membro para fora). O navegador do membro começa nas suas pastas, as do hub e as de nós (`Nome do nó · projects`), e de uma pasta nomeada o "subir" volta a essa lista. Um membro só tem as tools de arquivo na pasta; o `shell` só se o dono liberou a tool para ele, e como ele não tem quem aprove, o comando é recusado.
- **Web.** O `FolderPicker` ganhou o seletor de **Máquina** (Hub ou um nó online, aprovado, ligado e com pasta), só para o dono (o membro não lista nós; as dele já vêm na lista). A etiqueta mostra `pasta · nome do nó`.
- **A tela do dono (Sessão 134).** Em Pessoas, o botão "Pastas de trabalho" de cada membro edita `workdirs` (uma por linha, caminho absoluto) e `node_workdirs` (uma por linha, `id-do-nó:pasta`), com a chave de pareamento como as outras mudanças. `ClientMessage::SetUserWorkdirs` → `UserChange::SetWorkdirs` → `warden_bootstrap::users::set_user_workdirs`, que valida tudo (caminho absoluto sem `..`; caminho de nó só com nomes simples; id de nó sem `:`), tira repetidos e **recusa a mudança inteira** se uma linha for ruim. `UserInfoDto` ganhou `workdirs` e `node_workdirs` (`NodeFolderDto`). O desktop tem o comando `set_person_workdirs` (como `set_person_tools`), **mas não há tela de pessoas nele**, então só a web tem a tela.
- **CLI (Sessão 135).** `/folder <caminho>` escolhe a pasta da sessão do terminal (`/folder` mostra, `/folder off` tira), **só antes da primeira mensagem** (`turn_count == 0`): o caminho é resolvido (`canonicalize`), tem de ser uma pasta que existe, e a cada turno o CLI faz `scope_to_workdir` na sessão (uma pasta que sumiu para o turno com erro, sem recuar para o cofre). É a pasta local do computador, como no desktop: o CLI não tem hub nem nós. A sessão do terminal não grava conversa, então não há `workdir` guardado.
- **Extensão do navegador (Sessão 136).** O painel lateral ganhou a pasta de trabalho: o background guarda `workdir` (como o `agentId`), `selectWorkdir` só vale **antes da primeira mensagem** (uma conversa que já está no hub recusa) e `sendChat` só leva a pasta no turno que **cria** a conversa; abrir uma conversa restaura a pasta dela, e uma conversa nova começa sem pasta. O `FolderPicker` do painel é **inline** (o painel é estreito) e fala com o background (`listDirs` → `ListDirsResponse`), que faz o `ListDirs` no hub; serve a pasta do hub e, para um membro, as pastas de nós que o dono nomeou. **O painel não lista nós** (não há `ListNodes` nele), então o dono não escolhe uma pasta de nó por aqui: só vê, pelo nome do id, uma que já foi escolhida em outro cliente.
- **Celular (Sessão 143).** O app Flutter fala WebSocket direto em Dart, então o seletor é Dart puro, sem a ponte Rust: `ListDirsMessage`/`DirListMessage`/`DirErrorMessage`, `ChatMessage.workdir` e `ConversationSummary.workdir` espelham o protocolo (os literais dos testes são os do teste Rust do hub). `ChatTranscript` guarda `workdir` com as mesmas regras da web e da extensão: `canPickFolder` só para uma conversa que o hub ainda não tem e sem mensagem, a pasta só viaja na mensagem que **cria** a conversa, abrir uma conversa restaura a dela e uma nova começa **sem pasta** (ao contrário do agente, que a nova herda). Uma faixa acima da conversa (`_FolderBar`) tem o botão "Working folder: none"/o nome da pasta, com um X para tirar, e vira uma etiqueta fixa depois da primeira mensagem; o navegador é uma folha de baixo (`folder_picker.dart`) com "Up", subpastas e "Use this folder", desligado no topo da lista de um membro. **Só pastas do hub**: o celular não lista nós (o `listDirs` do `ServerConnection` só leva `path`), então um membro com `node_workdirs` não as escolhe aqui.
- **Fica**: o opencode numa pasta de nó, o desktop ligado a um hub, o seletor de nós **numa tela** (verificado só pelo hub e pelo `tsc`), o celular sem pastas de nós, e nós antigos sem `list_dirs` (o navegador mostra o erro do nó).

## O desktop como cliente de um hub, além de servidor (P102, Sessão 143)

**Decisão do usuário**: o desktop é **cliente e servidor ao mesmo tempo** (já era servidor, com o hub embutido, e nó, com o "emprestar este computador"), **tanto pela interface web do hub quanto pela interface nativa**; a web de cada hub abre em **janela separada por hub** (descartado: trocar a janela principal, que dividiria o armazenamento do navegador com o app local). Plano aprovado em duas fases.

**Achados que mandaram no desenho**: a UI do desktop chama `invoke(...)` direto em ~100 pontos, sem camada de abstração, e o chat é um `send_message` bloqueante com os eventos `chat-event`, `approval-request` e `conversations-changed`; a web é um cliente de hub completo (`web/src/hub/connection.ts`); o cliente Rust `ServerConnection` só faz conexão, handshake e send/recv (sem correlação por `request_id`, heartbeat, reconexão nem TruthID); o hub manda `X-Frame-Options: DENY` (então a web dele não cabe num iframe, só numa janela de topo) e não tem CSP nem CORS.

**Fase 1 (feita)**: hubs salvos e uma janela por hub.
- `warden_bootstrap::saved_hubs` (lógica pura, testada sem o Tauri): `hubs.json` ao lado do `config.toml`, `0600`, só `{ id, name, url }` — **nunca** chave, senha nem token. `normalize_hub_url`: `http`/`https` ficam, `ws`/`wss` viram `http`/`https`, endereço sem esquema vira `http`, o resto depois de host e porta cai (a interface fica na raiz), e esquema que não é de hub, endereço sem host e usuário ou senha no endereço são recusados (sem repetir a senha na mensagem). Um endereço só pode existir uma vez (digitado de outro jeito é o mesmo hub), nome obrigatório de até 60 caracteres, no máximo 50 hubs, e um arquivo quebrado é erro, não lista vazia (um `save` não o sobrescreve). `ensure` devolve o hub que já tem aquele endereço (o atalho do hub embutido não empilha entradas).
- `desktop/src-tauri/src/hub_cmds.rs`: `list_hubs`, `save_hub`, `ensure_hub`, `remove_hub`, `open_hub_window` (cria `WebviewWindowBuilder` com o id do hub como rótulo e `WebviewUrl::External`, ou traz a janela aberta para a frente; async de propósito, porque criar janela de um comando síncrono pode travar). **A janela de um hub não recebe IPC**: `capabilities/default.json` vale só para a janela `main`, então uma página que vem de outra máquina não alcança o `invoke` do app. A identidade do aparelho e o token ficam no `localStorage` da própria janela (a origem é o hub, então um por hub).
- Tela: Workspace → "Hubs" (`HubsSection.tsx`), com a descoberta de LAN reaproveitada (`discover_hubs`) e o atalho "Open this computer's own hub in a window" quando o hub embutido roda com `webUrl`.
- **Desvio do plano**: a lógica pura ficou em `warden-bootstrap` e não no módulo do desktop, para testar sem compilar o Tauri e para a Fase 2 reaproveitar.

**Fase 2, commit A (feito, Sessão 143): o ator `RemoteHub`.** Decisão: **ator em Rust, não o cliente TypeScript da web no webview** — um `ws://` de rede local a partir da origem `tauri://` pode cair em bloqueio de conteúdo misto, o navegador não aceita certificado autoassinado nem CA própria, e **só o Rust dá para provar aqui, sem janela**, com um teste de integração contra um hub real. `warden_server::remote_client`: um task é o dono do `ServerConnection` (sem `split`) e o resto fala com ele por um `RemoteHandle`. **Pedidos** levam `request_id` e a resposta é achada sem nomear cada variante (lê `requestId` do JSON da mensagem); **um turno (`Chat`) não tem `request_id`**, a resposta se casa por `conversation_id` (se vier sem, e só há um turno em andamento, é dele), **turnos de conversas diferentes rodam juntos e os da mesma ficam em fila**, porque o hub não protege dois turnos de uma conversa disputando o arquivo. O que o hub empurra (`ChatEvent`, `ApprovalRequest`/`Cancelled`, `ConversationsChanged`) e o estado (`Connecting | Connected{user} | Retrying | Stopped`) saem por um `RemoteSink`; o desktop os transforma nos **mesmos eventos do motor local**, então as telas não mudam. A chave de pareamento ou a senha só servem no primeiro login: o hub entrega um token e é ele que reconecta (1 s dobrando até 60 s); `AuthRejected` encerra (retentar não resolve) e o desktop esquece o token. **Identidade própria** (`remote_hub.json`, um `desktop-<nome>-<8hex>` e um token por hub, `0600`, nunca chave nem senha), não a do nó (`node.json`): o mesmo aparelho seria nó e cliente de um hub e parear de novo zera o status. O desktop não mostra código de recuperação, então o membro entra sem pedir a criação da chave de dados (`recovery_codes: false`), e a conta que o hub avisa como `locked` só abre com a senha. Aprovação do hub: o id do hub é **por conexão** e colidiria com os locais, então o `TauriSink` pede um id ao `ApprovalBroker` (`allocate`), guarda o par e o `resolve_approval` responde ao hub com o id dele; quando a conexão cai, as aprovações abertas são fechadas para a pessoa (o hub conta como não). `ServerConnection::handshake_with_token` reconecta só com o token **e devolve o `UserInfoDto`**. **Limites do hub que ficam**: `ChatEvent` só existe em projeto de código do dono, `CancelTurn` só para esses, e `Chat` não leva `providerId`.

**Fase 2, commit B (feito, Sessão 143): a interface nativa em modo remoto.** Um hub escolhido troca **só** o que mostra a lista de conversas, os projetos e os agentes do chat; o resto das telas (Settings, Skills, Vault, Usage, Tarefas, Webhooks, Sync, Workspace) segue sendo deste computador (limite aceito, a fatia "resto das telas"). **Camadas**: `lib/hubMap.ts` é puro (sem Tauri nem React) e testado com o Node (`npm test`), `lib/hub.ts` só chama `remote_connect`/`remote_disconnect`/`remote_request`/`remote_chat`/`remote_send`, e o `App.tsx` decide qual máquina está em uso (`activeHubId`; `null` é este computador) e ramifica nos pontos onde antes só havia o motor local. **Regras do modo remoto**: o hub guarda a conversa, então nada é gravado daqui (`persist=false`) e a **cópia do hub** (o histórico relido) substitui a mostrada depois do turno, com o `usage` e o aviso de reserva do turno postos na última resposta (o histórico não os guarda); um turno **não leva `history` nem modelo** (o hub escolhe pelo agente) e **só a mensagem que cria a conversa leva o projeto ou a pasta**, nunca os dois; uma conversa que esta tela acabou de criar fica no topo da lista até o hub a listar (`startedHere`); o modo de código é dito antes de cada tarefa de código (o hub o esquece ao reiniciar); `conversations-changed` relista e relê a conversa aberta. **Aprovações** passam pelo mesmo `ApprovalModal` e por `resolve_approval`: o Rust troca o id do hub por um id local (ver o commit A). **Máquina e conexão**: ao escolher um hub tenta-se conectar com o token; sem token (`remote_connect` diz "not signed in") abre o diálogo (chave de pareamento ou usuário e senha); uma chave errada mostra o motivo **sem trocar de máquina**; a chave e a senha vão ao Rust uma vez e não ficam na página. O estado da conexão vem pelo evento `remote-hub-state` e aparece sob o seletor (conectado como o dono ou como quem; conta **trancada** e **senha provisória** explicadas, porque o desktop não as resolve; tentando de novo; desconectado, com "Sign in again" quando o hub recusou o aparelho). Num hub o chat **não mostra o seletor de modelo** (o agente decide) **nem a pasta deste computador** (o commit C traz a do hub). **Contrato testado**: o simulador do hub nas checagens de tela foi escrito à mão, então um teste em Rust (`remote_cmds.rs`) serializa as mensagens reais do hub e confere os nomes que os mapeadores leem e as que a tela envia.

**Fase 2, commit C (feito, Sessão 143): a pasta de trabalho do hub no chat nativo.** O botão da pasta do `ChatArea` escolhe o navegador por onde a conversa roda: no modo local, o diálogo nativo do sistema; com um hub ativo (`hubFolders`), o `FolderPicker` (porte do da web) sobre `ListDirs`. O dono pode escolher também **uma pasta de nó** (o navegador ganha um seletor "Machine" com os nós usáveis: online, aprovados, ligados e que emprestam arquivos), e o caminho viaja como `node:<id>:<caminho relativo>`; o membro só vê as pastas que o dono lhe deu (a lista dele começa vazia de caminho, "Your folders", e subir da pasta dele volta a ela) e **o app nunca pede `listNodes` por ele**. A regra não muda: escolhida antes da primeira mensagem, fixa depois, e só a mensagem que cria a conversa a leva (e nunca com um projeto). O rótulo usa `folderLabel`/`folderPlace` (uma pasta de nó aparece com o nome da máquina; os nós são lidos quando a conversa aberta tem uma). Um primeiro `ListDirs` que falha mostra **o motivo do hub** (o porte não repete a frase genérica da web). **Com isto o P102 não tem mais código a fazer**: o que resta são testes que ninguém viu rodar (o app Tauri aberto, um hub e um nó reais juntos, um modelo real na pasta, o shell pedindo aprovação).

**Fatia "resto das telas" (Sessão 144): Cofre, Uso, Skills, Tarefas e Webhooks pelo hub.** O desenho: cada tela recebe `remote` do `App` (e é remontada por `key` ao trocar de máquina) e fala com uma interface de pequenas funções, uma implementação em `invoke` e outra no `lib/hub.ts` sobre o mesmo `remote_request`; os mapeadores puros ficam no `hubMap.ts`. **Chave de pareamento**: o hub a pede de novo em toda mudança de tarefa e de webhook, e o app nunca a guarda (a identidade do hub é só o token); **decisão do usuário: pedir a cada mudança**, num diálogo (`usePairingKey`) que devolve a chave para aquela chamada e a descarta (cancelar é uma rejeição `KeyCancelled`, sem banner). O `vaultError` mantém o `conflict` (`expectVaultReply`) para a tela oferecer recarregar ou sobrescrever. O relatório de uso do hub (`requestUsage`, com o fuso do computador) é pedido uma vez por montagem e guardado até um limite ser estendido. **Fora do hub**: tokens por agente e por provedor, arquivos anexados e rascunho de skill por modelo, e o interruptor de rodar tarefas (que é uma opção do hub). **Fica local**: Settings, Sync e Workspace.

**Como a cola é testada sem janela (Sessão 143).** Três camadas, cada uma com o que as outras não veem: o **ator** contra um hub real (`warden-server/tests/remote_client.rs`, sem Tauri), as **telas** contra um simulador do hub (Brave headless, sem Rust), e **os comandos pelo IPC do Tauri contra um hub real** (`desktop/src-tauri/src/remote_ipc_test.rs`, que só não tem o webview). Para o terceiro, os comandos de `hub_cmds` e `remote_connect` são genéricos no runtime (o app roda no `Wry`, o teste no `MockRuntime`) e os caminhos que o desktop lê da pasta de configuração passam por um ponto único, `config_paths.rs`, com um desvio que só existe em `cfg(test)`. O pedido de teste usa a origem `tauri://localhost`: em Linux `http://tauri.localhost` é remota e o ACL do Tauri recusa os comandos do app ("not allowed. Plugin not found").

**Fase 2 (esboço original; A, B e C estão feitos)**: um ator `RemoteHub` em Rust sobre o `ServerConnection` (com correlação por `request_id`, heartbeat, reconexão e fan-out de eventos) que reemite os **mesmos eventos** de hoje, para `liveTurn.ts` e `ApprovalModal` não mudarem; uma camada `api` no frontend que alterna "este computador" × hub ativo e manda para um `remote_request` genérico os comandos que têm equivalente em `ClientMessage`; só `deviceId` e token por hub no disco. Fatias: (a) conexão e login (chave de pareamento e membro; TruthID exige estender o cliente Rust); (b) conversas, histórico, chat, aprovações, projetos e o **seletor de pasta via `ListDirs`** (fecha o P102; em modo remoto pula os `append_conversation_messages`, porque o hub grava sozinho); (c) skills, cofre, tarefas, uso, configurações, webhooks e pessoas. Lacunas já vistas: `ChatEvent` em streaming só existe em projeto de código, `providerId` não está em `Chat`, `task_history` e `webhook_history` não têm mensagem equivalente.

## Webhooks de entrada (P105, Sessão 141)

Um **webhook** é um gatilho nomeado que roda um agente com um prompt, como uma tarefa agendada, mas disparado por um `POST` com token em vez do relógio ("quando o build falhar, diga por quê"). Lacuna nº 2 do estudo do OpenClaw (`STUDIES.md`). Decisões: **separado de `[[tasks]]`** (um quarto tipo de gatilho mexeria no agendador, no `manage_tasks` e nos formulários); resposta **assíncrona** (`202` na hora, o resultado vai para a conversa); autenticação **por token** (Sessão 141) **ou por assinatura HMAC** (Sessão 142, seção "Assinatura HMAC e administração pelos clientes" abaixo). A Sessão 141 foi só backend e CLI; as telas vêm depois.

- **Definição.** `[[webhooks]]` no `config.toml` (`WebhookConfig` em `warden-bootstrap/src/webhooks.rs`): `id` (1 a 54 letras, dígitos, `-` ou `_`: a conversa é `task-hook-<id>` e um id de conversa tem no máximo 64 caracteres), `agent` (opcional, só agente do dono), `prompt`, `enabled`. `check_webhooks` valida a lista; `upsert_webhook` valida a lista inteira antes de aplicar. Como a conversa `task-hook-<id>` também é a de uma tarefa chamada `hook-<id>`, `check_task_clashes` recusa a colisão **dos dois lados** (ao criar o webhook, e em `upsert_task`, que a ferramenta `manage_tasks` também usa, e no `tasks add` do CLI).
- **Tokens** (`warden-server/src/webhook_tokens.rs`, `~/.config/warden/webhook_tokens.json`, **fora do config que sincroniza**): `whk_` + 64 hex, **um por webhook**, mostrado uma vez; só o SHA-256 vai ao disco (arquivo `0600`), comparado em tempo constante (`keys_match`). Um token abre **só o webhook dele**: um que vaze dispara um prompt, não o hub. Um token novo para o mesmo webhook **substitui** o anterior (é a rotação). O arquivo é relido a cada chamada, então rotacionar ou revogar pelo CLI vale na próxima requisição. Como a URL é de um hub e os tokens são locais, só o hub chamado executa: não precisa da chave `--run-tasks`.
- **Rota** (`warden-server/src/webhooks.rs`, ligada em `server.rs::serve_web_or_ws` ao lado do `/v1/`): `POST /hooks/<id>` com `Authorization: Bearer whk_…` ou `X-Warden-Token` (para serviço que não controla o cabeçalho). Sem `with_webhooks` o prefixo responde `404` (**nunca** cai na página web, cujo fallback devolveria `200` a uma chamada). A ordem importa, porque quem chama é um estranho até o token ser conferido:
  1. o método (só `POST`, senão `405`) e o **token, antes de ler o corpo**: sem token ninguém faz o hub aceitar 256 KiB. Token errado, token de outro webhook, id que não existe e id impossível dão **o mesmo `401`**, depois de `WRONG_KEY_DELAY` (1 s, a taxa de chute da chave de pareamento), para a resposta não dizer quais ids existem;
  2. o webhook no config (relido a cada chamada, então pausar ou remover vale na hora): sumiu → `404`, pausado → `403`;
  3. o corpo: `Content-Length` obrigatório (`411` sem ele ou com `chunked`), número inválido → `400`, mais de **256 KiB** → `413` já pelo cabeçalho, corpo curto → `400`;
  4. a execução: **uma por vez por webhook** (`WebhookRunner`, conjunto `running` compartilhado por todas as conexões); uma chamada enquanto a anterior trabalha é `409`, **nunca enfileirada**.
  Sucesso: `202 {"status":"started","webhook":"<id>","conversation":"task-hook-<id>"}`.
- **A execução** (`run_webhook`, no mesmo núcleo das tarefas: `tasks::run_unattended_turn`, extraído do `run_task`): orquestrador do hub escopado ao agente e ao modelo dele, **sem approver** (tool que pede sim é recusada, ninguém olha) e sem `message_agent`, as últimas 20 mensagens como histórico, gasto no canal **`webhooks`** com o usuário `webhook:<id>` (os limites do P4 valem: um limite de escopo `channel webhooks` para a chamada seguinte). O resultado, ou o motivo de não haver, entra na conversa `task-hook-<id>`, na pasta das conversas das tarefas (então todo aparelho a lista) e o hub avisa os aparelhos com `ConversationsChanged`. Essa conversa também fica de fora do aprendizado (prefixo `task-`, `learning.rs`). O relatório de gasto conta os webhooks na mesma linha das tarefas, que passou a se chamar **"Tarefas e webhooks"**.
- **O corpo é dado de fora** (`input_for`): a mensagem é `[Webhook '<id>', <hora>]`, o prompt do webhook, e depois o corpo **entre duas linhas de cerca aleatórias por chamada** (nada no corpo consegue fechá-la antes), com o tipo de conteúdo (sem quebra de linha), o tamanho e a instrução de tratá-lo como dados e nunca como comandos. O modelo vê no máximo **32 KiB** (cortados por caractere, nunca no meio de um; bytes que não são UTF-8 viram `U+FFFD`), e o texto diz quando cortou. **Isso reduz a injeção de prompt, não a elimina**: o que limita de verdade uma chamada é o `allowed_tools` do agente, a recusa de aprovações e os limites de gasto. Recomendação: aponte o webhook para um agente com poucas tools.
- **CLI**: `warden-server webhooks list|add|pause|resume|remove|token|revoke`. `add` valida e salva pelo `save_config` (mantém os comentários); `token <id>` imprime o token uma vez (e diz o `curl`), `revoke` tira só o token, `remove` tira o webhook e o token (a conversa fica).
- **Testes**: unitários do `webhooks.rs` (validação e colisão com tarefa, cerca e corte do corpo, execução e nota de erro) e do `webhook_tokens.rs`; `crates/warden-server/tests/webhooks.rs` com um hub de verdade e HTTP por TCP (caminho feliz com a conversa, o aviso ao aparelho e a listagem; os seis jeitos de falhar o token com o mesmo `401`; token antes do corpo; pausado, removido e retomado sem reiniciar; os limites do corpo; o `409` e a volta; rotação e revogação; o limite de gasto no canal `webhooks`; o `404` sem a página web). **Mutações** que o teste pega: tirar a comparação do token, a checagem de pausado e a trava de uma chamada por vez. Também rodado o `serve` de verdade com `curl` (401 sem token, 202 com token, conversa gravada).
- **Fica**: um modo que espera a resposta do agente; um limite de chamadas por segundo além do `409`. Um token tem só o hash no arquivo, mas **quem vir o token** (num log de CI, por exemplo) dispara aquele webhook até ele ser rotacionado.

### Assinatura HMAC e administração pelos clientes (Sessão 142)

Para serviços que **assinam** o que mandam (GitHub, Gitea, Forgejo, Stripe) e não deixam escolher um cabeçalho `Authorization`. Decisões do usuário: **GitHub e Stripe agora, Slack depois**; **um modo por webhook** (token ou HMAC, não os dois); o segredo fica **em texto puro** no arquivo `0600`; **a web e o desktop ganham a tela** (a Sessão 142 faz primeiro o backend e o protocolo).

- **Modo.** `auth = "hmac"` no `[[webhooks]]` (`WebhookAuth`; ausente é `token`, então nada que existia muda, e só o modo `hmac` é gravado). No CLI, `warden-server webhooks add <id> --prompt ... --auth hmac`; o `token <id>` cria o que o webhook pede (um token `whk_…`, ou um **segredo de assinatura** `whsec_…`).
- **Credenciais** (`webhook_tokens.rs`). Cada entrada tem `kind` (`token` ou `hmac`; ausente é token, então um arquivo antigo continua valendo). Token: só o SHA-256. Segredo: **em texto puro** (`secret`), porque conferir um HMAC é fazer um; o arquivo é `0600` e **quem o ler consegue assinar chamadas** — esse é o preço de aceitar assinaturas. Um segredo também aparece uma vez; um novo substitui o anterior. `rename` leva a credencial junto quando o webhook muda de nome (sem nunca ficar duas).
- **Formatos** (`webhook_signature.rs`, crate `hmac` 0.12 com o `sha2` 0.10): **GitHub** `X-Hub-Signature-256: sha256=<hex>`, o HMAC-SHA256 do corpo cru; **Stripe** `Stripe-Signature: t=<segundos>,v1=<hex>[,v1=...]`, o HMAC de `"<t>.<corpo>"`, com tolerância de **5 minutos** (o tempo é parte do que se assina, então uma chamada velha, ou um `t` novo colado numa assinatura velha, é recusada; vários `v1` são o que o serviço manda ao trocar de segredo, vale qualquer um). Se os dois cabeçalhos vêm, **o do GitHub decide**. A comparação é em tempo constante (`verify_slice`), e um cabeçalho malformado é "não assinado": o mesmo `401`. Vetores de teste feitos com o `hmac` do Python (e o exemplo da documentação do GitHub), não com o próprio crate.
- **Ordem da chamada** (`webhooks.rs`). O modo vem da **credencial que o webhook tem**, não do que o chamador diz. Token: a prova vem antes de ler o corpo, como antes. Assinatura: só dá para conferir sobre o corpo, então **o corpo é lido antes** (mesmo teto de 256 KiB e 30 s), e só para um webhook que tem um segredo; um id desconhecido continua dando `401` sem ler nada. **Limite documentado**: por isso o tempo de resposta de um id com segredo difere do de um desconhecido pela leitura do corpo, e quem sonda pode inferir que existe. Depois da prova, o `auth` do config tem de concordar com o tipo da credencial (senão o mesmo `401`: o modo foi trocado e ninguém fez uma credencial nova).
- **Entrega repetida.** O formato do GitHub não leva tempo, então uma requisição gravada poderia ser reenviada; o GitHub também reenvia quando acha que falhou. Numa chamada **assinada**, o id de entrega (`X-GitHub-Delivery`, ou `Idempotency-Key`) já visto na última hora é reconhecido e **não roda de novo** (`202 {"status":"duplicate"}`). O id só é acreditado porque a assinatura foi conferida antes (uma chamada forjada não consome um id), é lembrado só depois de a execução começar de verdade (um `409` pode voltar), e a memória é do processo (1024 ids, 1 hora).
- **Administração pelos clientes** (`webhook_admin.rs`, para as telas): `ListWebhooks` (só o dono; um membro é recusado antes de tudo, `member_refusal`), e, **com a chave de pareamento**, a mesma espera de 1 s e o mesmo `lock` das tarefas, `SaveWebhook` (cria ou troca, com renomear), `SetWebhookEnabled`, `DeleteWebhook`, `CreateWebhookCredential` e `RevokeWebhookCredential`. A resposta é `WebhookList { webhooks, servesHere }` ou, para uma credencial nova, `WebhookCreated { id, credential, kind, webhooks }` — a única vez em que ela viaja. `WebhookInfoDto` diz o que o webhook quer (`auth`) e o que tem (`credential`: `token`, `hmac` ou nada), os primeiros caracteres, quando foi criada e usada e o id da conversa. Efeitos que os testes pegam: **renomear leva a credencial**; **mudar o `auth` na tela tira a credencial** (era do outro tipo); **apagar a tira**; editar sem mexer no modo a mantém.
- **Testes**: 5 unitários do `webhook_signature.rs`, 6 do `webhook_tokens.rs`, 2 novos do bootstrap e 1 de JSON do protocolo; no hub (`tests/webhooks.rs`, 20 casos no total), os de assinatura (GitHub, Stripe, os sete jeitos de falhar com o mesmo `401` de um id desconhecido, entrega repetida e id forjado, tipo da credencial) e os de administração pelo protocolo (fluxo ponta a ponta, mudança de modo, renomear, pausar, revogar, apagar, chave errada em cada mudança, formulário ruim, membro, hub sem webhooks). **Mutações** (aceitar qualquer assinatura, ignorar a janela de tempo, ignorar a entrega repetida, ignorar o tipo da credencial, não conferir a chave, manter a credencial ao mudar o modo, não levar a credencial na renomeação, não revogar ao apagar): cada uma derruba o teste certo.
- **Tela da web** (`WebhooksView.tsx`, aba **Webhooks**, só para o dono, depois de Tarefas): no molde da de Tarefas (`TasksView`). Lista cada webhook com o endereço (`POST <origem da página>/hooks/<id>`), o tipo (token ou assinatura HMAC), o agente e o estado da credencial ("sem credencial: não recebe chamadas", "a credencial é um X, mas o webhook quer Y: gere uma nova", ou o tipo, os primeiros caracteres, quando foi criada e o último uso). Ações: abrir a conversa (`task-hook-<id>`), gerar ou trocar a credencial ("Gerar token" / "Gerar segredo", "Trocar…" se já há uma, com o aviso de que a antiga morre na hora), revogar, pausar ou retomar, editar e apagar. **Toda mudança pede a chave de pareamento**, como em Tarefas. A credencial nova aparece **uma vez**, num cartão com o botão de copiar (as mesmas classes `api-key-created`/`api-key-value` da tela das chaves da API) e as instruções do tipo: o `curl` com o token, ou onde colar o segredo (o *Secret* do webhook do GitHub, com Content type `application/json`; o segredo do endpoint no Stripe). O formulário avisa que mudar o tipo apaga a credencial atual, e que o corpo da chamada vem de fora (dê ao webhook um agente com poucas tools). `connection.ts` ganhou `listWebhooks`, `saveWebhook`, `setWebhookEnabled`, `deleteWebhook`, `createWebhookCredential`, `revokeWebhookCredential` e o erro `WebhookError` (com `authRejected`); `messages.ts`, os tipos `Webhook`/`WebhookInfo` e as três respostas.
- **Verificação da web sem navegador**: o `connection.ts` **real** da web, compilado com o `esbuild`, contra um hub de verdade (o `startHub` do `e2e/harness.mjs`): os 9 passos do fluxo (lista, chave errada, salvar, credencial uma vez, chamada com token e o aviso na conexão, troca de tipo com uma assinatura feita pelo `node:crypto` — **uma implementação independente da do hub** —, entrega repetida, renomear, pausar, revogar, apagar, formulário ruim). Feito num script avulso, **fora do repositório**: os `e2e` que existem abrem um navegador.
- **Tela do desktop** (`WebhooksView.tsx`, botão **Webhooks** na barra lateral, depois de Tasks; em inglês como o resto do desktop): a mesma lista, formulário e ações da web, na `config.toml` e no `webhook_tokens.json` desta máquina, **sem pedir chave** (é a máquina do dono, como Tasks e Settings). Comandos Tauri em `webhook_cmds.rs` (`list_webhooks`, `save_webhook`, `set_webhook_enabled_cmd`, `delete_webhook`, `create_webhook_credential`, `revoke_webhook_credential`, `webhook_history`), **finos sobre `warden_server::webhook_admin::apply_webhook_change`**, a função que o hub também usa depois de conferir a chave e a trava, então as regras (renomear leva a credencial, mudar o tipo a tira, apagar não deixa nada) **não podem divergir** entre o hub e o desktop. Mostra o endereço pelo qual o hub embutido é alcançado (`EmbeddedServerHandle::base_url`: `http(s)://host:port`; `localhost` quando o hub escuta em todas as interfaces; nada com TLS sem nome de host) e avisa quando o hub embutido está desligado (as chamadas só chegam com ele ligado). "Last result" lê a conversa `task-hook-<id>` desta máquina; a lista olha de novo a cada 5 s (as chamadas vêm de fora). A credencial nova aparece **uma vez**, com copiar e as instruções do tipo, como na web.
- **O hub embutido do desktop agora serve `/hooks/`** (`server_cmds.rs`, `.with_webhooks(default_webhook_tokens_path())`; antes só o `warden-server serve` servia, então a tela existiria sem que nada atendesse). O teste do hub embutido (`start_embedded_server_inner_...`) chama `POST /hooks/<id>` sem credencial e exige o `401` do hub: **sem a ligação, ele vê o `404` da página web** (mutação feita e desfeita).
- **Salvar as configurações no desktop não apaga os webhooks** (Sessão 141: o `save_settings` reconstrói o `FileConfig` e copia `webhooks: existing.webhooks`), **ainda sem teste** (`save_settings` é um comando Tauri com estado).
- **Testes do desktop**: o payload JSON de cada comando (as chaves em camelCase que o TypeScript lê, com o que falta omitido e não `null`), o endereço base do hub embutido nos quatro casos, a função compartilhada chamada **direto, como o desktop a chama** (sem hub, sem chave, sem trava) e o `401` do hub embutido.
- **Fica**: o Slack (`X-Slack-Signature`); as duas telas **vistas** numa janela (só `tsc`, os builds e os testes de código foram rodados nelas); um teste de `save_settings` que prove que os campos preservados (`tasks`, `webhooks`, `nodes`...) sobrevivem; celular e extensão não têm tela de webhook.


## Autonomia por agente, níveis 1 a 4 (P122, Sessão 145)

Cada agente tem um nível (`AgentConfig.autonomy`, 1 a 4, padrão **4** = o comportamento de antes do campo) que diz quanto ele faz sem pedir. Um **único ponto** aplica o nível: `autonomy::authorize`, chamado por `Orchestrator::run_tool` antes de qualquer `tool.call`, **por cima** do que cada tool já pede sozinha (SSH com `require_approval`, `manage_agents`, `manage_tasks`).

- **1 só responde**: o orquestrador fica sem tools (`with_autonomy` esvazia a lista; o `scope_to_agent` também não anexa as tools opt-in). **2 sugere**: as tools só de leitura rodam e as outras voltam ao modelo como `error: ... only suggests ...`, para ele propor em texto. **3 pede antes**: as de leitura rodam e as outras perguntam pelo `Approver` do turno (`action = "tool_call"`, `target` = a tool, `detail` = os argumentos, cortados em 500 caracteres, "sempre permitir" oferecido com o nome da tool). **Sem aprovador** (Telegram, WhatsApp, celular, MCP, sub-agente) ou sem resposta em 120 s, **recusa**. **4**: como sempre.
- "Só de leitura" = `SAFE_AGENT_TOOLS` (`read_file`, `use_skill`, `read_skill_file`, `usage_stats`, `budget`, `generate_document`), passado pelo `scope_to_agent`; o core não conhece essa lista.
- **O nível só desce**: `with_autonomy` guarda `min(atual, pedido)`. `delegate_task` repassa o nível ao orquestrador aninhado (`Tool::with_autonomy`), e o `scope_to_agent` aplica o nível **antes** de montar os alvos do `delegate_to_agent`, que ainda recebem o próprio nível por cima: um agente de nível 2 não executa através de um sub-agente de nível 4.
- **Agente criado por outro agente** (`manage_agents`) nasce no **3** e nunca acima do nível de quem o criou (`with_caller_autonomy`); o cartão de aprovação mostra o nível. Membro (P84) não escolhe o nível do próprio agente: fica em 4.
- **Config**: `#[serde(default = "default_autonomy")]` (o arquivo antigo carrega como 4); `check_agents` recusa fora de 1 a 4. DTO do hub `AgentSettingsDto.autonomy` (`#[serde(default)]`), `AgentPayload` do desktop, seletor nas telas de Settings do desktop e da web e pergunta no wizard `/agents` do CLI. Mobile e extensão não editam agentes.
- **Nível 3 com `manage_agents` pergunta duas vezes** (o nível, depois a mudança que a própria tool mostra): aceito por ora, é o preço de um ponto único de controle.

### Nível 5, gerenciar subordinados sem perguntar (P122, Sessão 180)

`Autonomy::Manager` (`AgentConfig.autonomy = 5`, aceito por `check_agents`, pelo wizard do CLI e pelos seletores do desktop e da web; os rótulos de desktop, web, extensão e celular conhecem o 5). **Decisão do usuário**: dentro do escopo, sem o "sim".

- **O que muda**: só o `manage_agents`. `ManageAgentsTool::manages_alone` (nível 5 **e** um chamador na organização, `with_caller`) pula a pergunta ao aprovador em `create`, `update` e `delete`. Sem chamador (o uso de antes da organização), o 5 não vale nada e tudo continua esperando a pessoa. Sem aprovador (Telegram, tarefa agendada), o 5 consegue e o 4 recusa ("this channel can't ask").
- **O que não muda**: `plan` roda antes e segura a mudança no escopo do gerente (só os subordinados, nunca ele, o superior ou um par) e no teto dele (tools e poderes que ele tem); ligar poderes (`can_*`) segue só de um humano, e uma lista de tools com `manage_agents` ou `delegate_to_agent` é recusada; o agente criado nasce no nível 3, só com tools de leitura e com todas as categorias ligadas. Quem edita o nível de um agente continua sendo só uma pessoa (o `update` não tem campo de autonomia).
- **Para o orquestrador o 5 é um 4**: `with_autonomy` guarda `min(atual, pedido)` e o teto do orquestrador é o 4, então `authorize` trata os dois igual e uma categoria que a pessoa ligou (`approval_required`, como `elevated_agent` e `delete_data` para `manage_agents`) **ainda pergunta** antes de a tool rodar; nesse caso a tool não pergunta de novo.
- **Limite aceito**: o gerente pode, sem perguntar, dar a um subordinado tools que ele mesmo tem (o teto é o gerente), inclusive trocar a lista de um que já age sozinho. O que ele não faz é passar do que tem.

### Permissões "iniciar tarefas" e "criar agentes temporários" (P122, Sessão 180)

Duas flags em `AgentConfig`, `can_start_tasks` e `can_create_workers` (**ligadas por padrão**: um config antigo carrega como sempre; não são escritas no arquivo enquanto ligadas). **Decisão do usuário**: duas flags, ligadas para os agentes de hoje e **desligadas para o agente que outro agente cria** (`manage_agents` e a árvore de `org_edit`); o agente de um membro nasce ligado, porque `delegate_task` e `jobs` são tools padrão de membro.

- **O que fazem**: `can_start_tasks` desligada tira a tool `jobs` do agente, e sem `jobs` o orquestrador não monta a fila (`attach_jobs`), então `background: true` some de `delegate_task` e de `delegate_to_agent`. `can_create_workers` desligada tira o `delegate_task` (anônimo ou com `name`). `delegate_to_agent` continua com a `can_delegate_to_agents` de sempre.
- **Onde se aplica** (`tools_without_permission` e `without_unpermitted_tools`, em `warden-bootstrap/src/lib.rs`): em `scope_to_agent` (o agente da conversa) e em `delegate_targets` (o agente alcançado por outro), **depois** da lista `allowed_tools`, então valem seja qual for a lista. Os sub-agentes aninhados são estreitados junto (`with_allowed_tools`).
- **Telas**: duas caixas no Settings do desktop e da web e duas perguntas no wizard `/agents` do CLI; `AgentSettingsDto` e `AgentPayload` ganharam os campos com `default = true`. O `agentFromHub` do desktop **não** transforma `false` em `true` (teste), porque salvar o formulário reativaria a permissão. A extensão e o celular só leem a árvore e não salvam Configurações, então ficaram como estão.
- **Limite**: ligar ou desligar uma das flags não tem aprovação nem categoria de risco; é só Settings, como as outras `can_*`.

### Aprovação por categoria de risco (P122, Sessão 146)

Um agente que age sozinho (nível 4) pode ter **tipos de ação** que ainda exigem um "sim": `AgentConfig.approval_required` (vazio por padrão = como sempre). As sete categorias (`warden_core::autonomy::Category`, ids em snake_case no config, no hub e nas telas): `delete_data`, `spend_money`, `critical_infra`, `external_message`, `publish_code`, `important_config`, `elevated_agent`.

- **Mesmo ponto de controle**: `autonomy::authorize` recebe a lista exigida e a categoria da chamada (`Orchestrator::run_tool` pergunta ao `Classifier`). Nível 4 + categoria listada → pergunta pelo `Approver` (`action = "tool_call"`, agora com `ApprovalRequest.category`); sem aprovador ou sem resposta em 120 s, recusa. Nível 3 já pergunta tudo (e agora diz a categoria quando há); 2 recusa; 1 sem tools. Chamada sem categoria no nível 4 roda como antes.
- **Quem classifica** (`warden-bootstrap/src/risk.rs`, `build_classifier`, o primeiro que responde vence): (1) o mapa do usuário `[[tool_categories]] tool = "slack__post" category = "external_message"` no `config.toml` (só por arquivo, sem tela); (2) a tabela das tools do Warden (`builtin_category`: `shell`, `ssh_exec/upload/download`, `node_shell`, `node_write_file` → `critical_infra`; `manage_agents` create/update → `elevated_agent`, delete → `delete_data`; `manage_tasks` create/update → `important_config`, delete → `delete_data`; `browser_click_element` e `browser_navigate` → `external_message`; o prefixo `<dispositivo>__` é ignorado); (3) as dicas que um servidor MCP dá (`McpTool::risk_hints`, do `annotations` do rmcp): `destructiveHint` → `delete_data`, aberto ao mundo e não só-leitura → `external_message`; só dica explícita conta, e quem diz `readOnlyHint` nunca é classificado. `spend_money` e `publish_code` não têm tool nativa: só chegam pelo mapa.
- **Herança**: `with_approval_rules` **une** as categorias (o nível faz `min`); `Tool::with_approval_rules` repassa ao orquestrador aninhado do `delegate_task`, e o alvo do `delegate_to_agent` leva as suas por cima das do chamador. Um sub-agente não tem aprovador: com categoria exigida, a chamada dele é **recusada** (fecha por padrão).
- **Criação**: agente criado por `manage_agents` nasce com **todas** as categorias ligadas (e no nível 3); membro (P84) fica vazio.
- **Telas de aprovação**: `ApprovalRequest.category` → `ServerMessage::ApprovalRequest.category` (omitido quando vazio) → `RemoteEvent::Approval` → `ApprovalPayload` do desktop → desktop, web, CLI (linha `categoria:`), celular (diálogo e texto da notificação) e extensão. Quem não conhece o campo só não o mostra.
- **Editores**: Settings do desktop e da web (uma caixa por categoria, "Always ask me before"), wizard `/agents` do CLI (lista de ids, marcador `[aprova: n]`), `AgentSettingsDto.approval_required` e `AgentPayload.approval_required` (ids desconhecidos recusados por `settings::categories_from_ids`).
- **Limites conhecidos**: tools emprestadas por um nó e anunciadas por um dispositivo perdem as dicas do MCP no `ToolSpec` (valem só a tabela e o mapa); as dicas são conselho do servidor, não garantia; um agente com `elevated_agent` ligado e `manage_agents` pergunta duas vezes (o gate e a própria tool).

## Organização de agentes: cargo, superior e a árvore (P120, Sessão 147)

Cada agente do dono pode ter um **cargo** (`AgentConfig.role`, texto livre) e um **superior** (`AgentConfig.reports_to`, o id de outro agente). Na Sessão 147 era só um retrato; **na Sessão 148 a hierarquia ganhou efeito** (seção "Escopo de autoridade", abaixo). O nível de autonomia, as categorias e `message_agent` seguem sem ler esses campos.

- **Regras** (`warden-bootstrap/src/org.rs`, `check_hierarchy`, chamada por `settings::check_agents`, o ponto central de desktop, hub e CLI): ninguém reporta a si mesmo, a um agente que não existe, nem em círculo (a mensagem mostra `a → b → a`); agente de membro (P84) fica fora (sem cargo, sem superior, e não pode ser superior). Cargo ou superior em branco vira `None`.
- **Cascatas**: apagar um agente (`remove_agent_from`, que o CLI, `manage_agents` e a tela usam) passa os subordinados dele ao **superior dele** (ou ao topo); renomear (`apply_hub_settings`, o wizard do CLI e as duas telas) leva o `reports_to` junto. As telas fazem a cascata no formulário antes de salvar; `check_agents` recusa o que sobrar pendurado.
- **A árvore**: `build_org` (raízes na ordem do arquivo, só agentes do dono; quem tem o superior sumido aparece no topo; um círculo que passasse não trava) e `render_org` (texto). Desktop: tela "Organization" no rodapé da barra lateral (`OrganizationView.tsx`, `lib/org.ts` com o mesmo `buildOrg`, `descendantsOf`, `renameInReports`, `removeFromOrg`); web: aba "Organização" só para o dono (`hub/org.ts`, lê `requestSettings`); CLI: `/agents tree` e o marcador `[reporta a x]` na lista. Cada nó mostra id, cargo e selos (delega, gerencia agentes, recados, tarefas, autonomia, categorias).
- **Editores**: cartão de agente no Settings do desktop e da web (Role e Reports to; o seletor não oferece o próprio agente nem os seus subordinados, para não fechar um círculo) e wizard `/agents` (cargo e superior). DTO `AgentSettingsDto.role`/`.reports_to` (omitidos quando vazios), `AgentPayload` do desktop.
- **Editar pela árvore** (Sessão 155): a tela "Organization" (desktop) e a aba da web deixam **dar cargo e superior** a um agente (o seletor não oferece o próprio nem os de baixo), **adicionar um subordinado** (sob um agente ou no topo) e **remover** (os subordinados sobem para o superior). Cada mudança é uma operação estreita, `AgentOrgEdit` (`setPosition`, `addReport`, `remove`), aplicada por `warden_bootstrap::org_edit::apply_org_edit` numa **cópia** dos agentes e validada por `check_hierarchy` (nada muda se recusar); só os agentes do dono entram, os de membros não. O agente novo nasce **cuidadoso**, igual ao que um gerente cria: ferramentas só de leitura, nível 3, todas as categorias a pedir, sem delegar nem gerenciar agentes (só uma pessoa liga isso, em Settings), no modelo padrão. Escreve só `[[agents]]` (e hosts SSH e nós que citavam o removido) e **reinicia o orquestrador**, para o alcance de quem gerencia seguir a árvore na hora. Hub: `EditAgentOrg { pairingKey, edit }`, só do dono (`member_refusal`), com a chave e o lock dos salvamentos, respondido como um salvar (`SettingsSaved` com as configurações novas, ou `SettingsError` com o motivo, e o arquivo volta ao que era se o hub não consegue subir). Desktop local: comando `edit_agent_org`, sem chave, que reinicia o motor do app e o do hub embutido. Arrastar na árvore **não** foi feito: o editar é por formulário.
- **Fica**: arrastar nós, a atividade de cada nó (conversa e tarefas já abrem, Sessão 156), mobile e extensão (sem tela de agentes). A cópia do `buildOrg` em TypeScript existe duas vezes (desktop e web), como o resto das duas telas.

### Escopo de autoridade (P120, Sessão 148)

**Um agente só modifica o que está no seu escopo**: os agentes que reportam a ele, direta ou indiretamente (`org::subordinates_of`, descendentes estritos, na ordem do arquivo). Nunca ele mesmo, o superior ou um par. A decisão do usuário foi só a subárvore (sem exceção para agentes sem superior) e o teto no próprio gerente.

- **`manage_agents`** conhece quem chama (`ManageAgentsTool::with_caller`, passado por `scope_to_agent`):
  - `list` mostra só o escopo (agora com `role` e `reports_to`).
  - `update` e `delete` recusam fora do escopo ("is not yours to edit/delete: it doesn't report to you"), **antes** de qualquer pergunta à pessoa.
  - Um subordinado com `can_delegate_to_agents` ou `can_manage_agents` só é do gerente **se o gerente também tem esse poder** (o teto é o gerente). A regra da Sessão 80 ("só um humano edita agente com poder") vale como antes para uma tool **sem chamador** e para quem não tem o poder.
  - `create` faz o agente novo reportar ao chamador (ou a um subordinado dele, com `reports_to`); `role` opcional; segue sem poderes, no nível 3 e com todas as categorias (P122).
  - `update` aceita `role` e `reports_to` (mover): o novo superior é o chamador ou alguém do escopo, nunca o próprio alvo nem um descendente dele. Apagar re-pai os subordinados do apagado. O cartão de aprovação mostra cargo e superior (antes → depois). **Ligar poderes continua só de um humano**, e toda mudança continua esperando o "sim" da pessoa.
- **`delegate_to_agent`** (`delegate_targets(config, orchestrator, caller)`): quem **está na hierarquia** (`org::is_in_hierarchy`: tem superior ou subordinados) delega só aos seus subordinados; quem está **fora** alcança todos, como antes. A lista "ao vivo" usa o mesmo filtro; um chamador sem subordinados fica sem a tool (o subordinado que ele criar no mesmo turno só é alcançável no turno seguinte). `message_agent` segue livre (comunicação lateral, como a visão pede).
- **Contornar**: a delegação já herda o nível de autonomia (mínimo) e as categorias exigidas (união) do chamador (P122), então um agente não ganha por um subordinado o que não tem.
- **Consequência a saber**: uma configuração **plana** (ninguém reporta a ninguém) com um agente que gerencia: ele cria, mas não edita nem apaga ninguém até a hierarquia existir. O agente que ele cria passa a ser dele.

## Tarefas de agentes: registro, estado, progresso e modelo por delegação (P123, Sessão 149)

Uma delegação **em segundo plano** (`delegate_task` ou `delegate_to_agent` com `background: true`) deixou de viver só na memória de um turno: vira um **registro** que sobrevive a ele, com responsável, objetivo, estado, modelo, resultado e tokens, visto numa tela.

- **Núcleo** (`warden-core/src/jobs.rs`): `TaskRecorder { created, running, finished }`, `TaskSpec`, `TaskOutcome { Done, Failed, Cancelled }`; `JobBoard::recording(max, recorder, TaskContext)` e `spawn_task(label, TaskDraft, work)`. A tarefa nasce **pendente** (na fila, esperando uma vaga do limite de paralelismo), passa a **em andamento** ao pegar a vaga e termina **concluída** (com os tokens do `MessageOutcome`, que antes eram descartados), **falhou** ou **cancelada**: um guarda de Drop dentro do futuro marca `Cancelled` quando o fim do turno aborta o job (`abort_unfinished`), inclusive o que ainda estava na fila. Sem recorder, `spawn_task` é um job comum. O orquestrador (`with_task_recorder`) dá a cada turno um **grupo** novo (`turn-<ns>-<n>`), o agente dono (`agent_id`) e o canal; é sobre o grupo que o progresso se conta. A tool `jobs list` passa a mostrar `task_id`, `assignee`, `model` e `total_tokens` das tarefas registradas.
- **Log** (`warden-bootstrap/src/agent_tasks.rs`): `agent_tasks.jsonl` ao lado do `spend_ledger.jsonl`, só anexando eventos (`task`, `running`, `finished`), então vários processos escrevem sem se atropelar. `read_agent_tasks` dobra os eventos (o último estado vence), ignora linha estragada e evento de tarefa desconhecida, devolve as **200 mais novas** e mostra como `cancelled` ("interrupted") o que ficou `pending`/`running` há mais de **6 h** (quem rodava sumiu). Passa de 1 MB, o arquivo é reescrito com um retrato por tarefa mantida. Objetivo cortado em 2000 caracteres e resultado em 4000. Um log que não grava nunca para o trabalho. `WARDEN_AGENT_TASKS` (um caminho, ou `off`) troca o lugar. O `bootstrap()` liga o `FileTaskRecorder` em todo canal.
- **Estados**: pendente, em andamento, **aguardando agente** (Sessão 150), concluído, falhou, cancelado. **pausado** (Sessão 154, uma pausa de verdade pedida pela tela). O **progresso é por grupo** (terminadas ÷ total do turno, contando a árvore toda).
- **Subtarefas e "aguardando agente"** (Sessão 150): a turn de uma tarefa em segundo plano pode começar **subtarefas** (gerente → tarefa → subtarefa, `MAX_TASK_DEPTH = 2`; a subtarefa não abre outras). `JobBoard::spawn_task_with` entrega ao trabalho um `TaskLink` (id da tarefa, grupo, responsável, profundidade, limite de paralelismo, recorder) e o delegador faz `orchestrator.with_parent_task(link)`; `attach_jobs` aceita então um orquestrador `charged` com vínculo e `depth < MAX_TASK_DEPTH`, e cria o quadro com `TaskContext { parent, depth }`: a subtarefa fica no **mesmo grupo**, com `TaskSpec.parent` = a tarefa pai e dono = o agente da tarefa (ou o responsável dela). **Sem recorder não há vínculo, logo não há aninhamento.** Quando o agente de uma tarefa chama `jobs result` com espera e a subtarefa ainda não terminou, `JobBoard::wait_as_parent` avisa `TaskRecorder::waiting(pai)` antes e `resumed(pai)` depois: a tarefa pai aparece como **aguardando agente** só nesse intervalo. O fim do turno pai derruba o quadro filho (`JobsGuard`) e cada subtarefa se marca `cancelled`. Cada quadro tem o seu limite de paralelismo (o do pai), então o paralelismo total pode multiplicar por nível; o teto de chamadas do turno (`TurnBudget`) segue valendo para a árvore toda. Log: `AgentTask.parent_id` e os eventos `waiting`/`resumed`; protocolo: `AgentTaskDto.parentId` e o estado `waiting`; as duas telas mostram as subtarefas recuadas sob a tarefa pai.
- **Agentes nomeados abrindo subtarefas** (Sessão 151): um agente alcançado por `delegate_to_agent` **em segundo plano** que tem `can_delegate_to_agents` ganha a **sua própria `delegate_to_agent`** no turno da tarefa dele, para delegar a outros agentes nomeados (gerente de programação → agente de backend). `NamedSubAgent.delegation` é um `DelegationSpawner` **preguiçoso**: o `delegate_targets` do bootstrap monta, por agente que pode delegar, uma função que chama `build_delegate_to_agent_tool(cópia da config, base do agente, Some(id))` e que só roda quando a tarefa roda (não se constroem as ferramentas de todos os agentes de uma vez). A base do agente leva o modelo, o nível de autonomia e as categorias **dele** (mínimo e união com os do chamador), mas todas as tools, para cada alvo dele ser limitado às suas. Só quando `link.depth < MAX_TASK_DEPTH` (o turno de nível 1, que ainda pode ter jobs): na delegação **síncrona**, sem log (sem vínculo) ou no nível 2 nada muda, e `jobs` precisa estar nas tools do agente. O alcance segue a hierarquia (P120): na organização, só os subordinados dele; fora dela, todos como antes. A subtarefa nasce com `parent_id` = a tarefa do agente, no mesmo grupo e com dono = o agente, e a tarefa dele fica "aguardando agente" enquanto espera.
- **Modelo por delegação**: `delegate_task` e `delegate_to_agent` aceitam `model` (id de provedor ou de combo; vazio = o padrão), resolvido por um `ModelResolver` que o bootstrap monta com `build_model_for` (`model_choices(config)`: só é oferecido com **dois ou mais** ids). Id desconhecido é recusado listando os válidos. O `delegate_task` aceita também `name`, o nome do **worker temporário**, que aparece como responsável. O modelo escolhido vale para esse turno do sub-agente (gasto e limites seguem pelas regras de sempre: P4, autonomia e categorias).
- **Telas**: `ClientMessage::ListAgentTasks` → `ServerMessage::AgentTaskList` (`AgentTaskDto`, só leitura, sem chave de pareamento; um membro recebe lista vazia), `Server::with_agent_tasks(path)` (o `serve` e o hub embutido do desktop usam o caminho padrão). Desktop: "Agent work" no rodapé (lê o arquivo pelo comando `list_agent_tasks` ou pergunta ao hub em uso), web: aba "Trabalho dos agentes" (só dono); grupos por turno com o agente dono, barra de progresso, uma linha por tarefa (responsável, objetivo, estado, modelo, tempo, tokens, resultado expansível), atualizando a cada 3 s. `lib/agentTasks.ts` (desktop) e `hub/agentTasks.ts` (web) têm o mesmo `groupTasks`.
- **Políticas nomeadas de modelo** (Sessão 153): `[[model_policies]]` no `config.toml` (`id`, `model`, `description`) dá nomes como "fast", "cheap", "reasoning", "code" ou "multimodal" a um provedor ou combo. O nome entra no enum de `model` **depois dos ids** e a descrição é mostrada ao agente (`ModelChoices.hints`); o `resolve` troca o nome pelo modelo antes de `build_model_for`. O que fica gravado na tarefa é o nome que o agente escolheu. Uma política que repete um id (ou outra política) ou aponta para um modelo que não existe é pulada com um aviso; um provedor ou combo renomeado leva a política junto e removido a leva embora; o Save do desktop preserva as políticas cujo modelo sobrou. Uma política vale como escolha mesmo com um provedor só (`model_choices` conta os nomes). **Só por `config.toml`, sem tela.**
- **Delegação síncrona registrada** (Sessão 153): `JobBoard::run_recorded` grava como tarefa (responsável, modelo, estado, tokens; falha e cancelamento também) a delegação **sem** `background`, quando o turno tem log. Não pega vaga na fila (quem chama já espera por ela, e uma vaga presa por tarefa que espera uma subtarefa pode travar), não entra na lista da tool `jobs` e não abre subtarefas (sem `TaskLink`). Sem log, roda como antes.
- **Pausar, retomar e parar pela tela** (Sessão 154): `warden-core::jobs` ganhou um registro por processo (`task_controls()`) das tarefas em segundo plano que estão rodando, com o `AbortHandle`, o `PauseGate` e o pai de cada uma. **Parar** (`cancel`) marca a tarefa e as de baixo como paradas por uma pessoa (`TaskOutcome::Stopped`, no log "stopped by a person") e aborta a raiz; derrubar o turno derruba o que ele abriu. **Pausar** liga o `PauseGate` da tarefa e das de baixo; o turno olha o portão **antes de cada chamada ao modelo** (`Orchestrator`, `TaskLink.gate`), então a tarefa termina a chamada em que está e espera. No log: estado novo `paused` (eventos `paused`/`unpaused`; uma espera por subtarefa não tira a pausa), e `waiting`/`resumed` seguem separados. Só controla tarefa **deste processo**: o hub ou o desktop que roda os turnos; tarefa de outra máquina ou de outro processo aparece sem botões (`controllable: false` no `AgentTaskDto`). Regras (`agent_tasks::control_agent_task`): pausar só quem está em andamento ou aguardando, retomar só quem está pausada, parar qualquer uma que não terminou; a tarefa que o turno esperava (síncrona) não é controlável. Protocolo: `ControlAgentTask { taskId, action: pause|resume|cancel }` com a chave de pareamento, só do dono, respondido pela lista nova ou `TaskError`; no desktop local o comando `control_agent_task` não pede chave. Telas: botões Pausar, Retomar e Parar na "Agent work" do desktop (que confirma o parar) e na aba da web (que pede a chave).
- **Limitar os modelos de um agente e ditar o modelo** (Sessão 156): `AgentConfig.delegation_models` (lista de ids de provedor, combo ou política; vazia = aberto, como antes). Com uma lista, o agente só vê esses ids no `model` de `delegate_task` e `delegate_to_agent` (`model_choices_for`, que corta o que não existe mais), e o **primeiro** é o `ModelChoices.default`: o modelo de uma delegação que não nomeia nenhum, **em vez do modelo do sub-agente**. Uma lista de **um** é a pessoa ditando o modelo de toda tarefa que o agente delega, e vale até com um provedor só (o "dois ou mais ids" é só para quem não tem limite). Lista sem nenhum id que exista oferece **nenhuma escolha** (roda no modelo do sub-agente), nunca tudo (falha fechado, com um aviso). Mecanismo: `Tool::with_model_choices` e `Orchestrator::with_model_choices`, aplicados por `scope_to_agent` ao `delegate_task` (e ao aninhado, que acompanha) e passados já prontos ao `delegate_to_agent` (`model_choices_of(config, caller)`). Vale para **quem escolhe**: o agente alcançado tem a lista dele quando delega por conta própria, e não herda a do chamador. Só uma pessoa liga (o gerente que cria um agente não a recebe, `manage_agents` e `org_edit` criam vazia, e o agente de membro não tem). Cascatas: renomear um provedor, combo ou política leva a lista junto; remover tira o id (e as políticas que ele respondia) das listas, e uma lista que esvazia volta a ser aberta; todo salvamento poda o que não existe (`prune_delegation_models`). O wizard `/agents` do CLI **não pergunta** isso, mas preserva a lista ao editar.
- **Tela das políticas** (Sessão 156): `ModelPolicyDto` no protocolo (`HubSettingsDto.model_policies`, `HubSettingsUpdate.model_policies`, ausente = mantém, tirando as de um modelo que o mesmo salvar removeu), `check_policies` (nome único e que não repete provedor nem combo, modelo que existe, descrição de uma linha de até 200 caracteres), a seção "Model policies" no Settings do desktop e "Políticas de modelo" na web, e o campo "Models for the tasks it delegates" no cartão de cada agente (lista ordenada, reaproveitando o editor de ordem dos combos). Os dois lados têm as cascatas em `lib/modelPolicies.ts` (desktop) e `hub/modelPolicies.ts` (web).
- **Atalhos por nó da árvore** (Sessão 156): "Chat" abre uma conversa nova já com aquele agente e "Tasks" abre o "Agent work" filtrado pelas tarefas **que o agente recebeu e que ele delegou** (`involvingAgent`); o filtro some ao abrir a tela pelo menu. No desktop o agente escolhido para uma conversa ainda não começada sobrevive ao recarregar das configurações (`newChatAgent`).
- **Wizard do CLI e o limite de modelos** (Sessão 157): `/agents` (criar e editar) pergunta `delegation_models` (`parse_delegation_models` valida contra provedores, combos e políticas, tira repetidos; em branco = aberto).
- **Parar a delegação síncrona** (Sessão 157): o registro de `task_controls()` guarda um `Runner` por tarefa: `Spawned(AbortHandle)` (a tarefa em segundo plano, como antes) ou `Inline { stop: Notify, finished }` (a delegação que o agente espera, de `run_recorded`). O `run_recorded` agora se registra e espera por `select!` entre o trabalho e o sinal de parar; parada, grava `Stopped` ("stopped by a person") e **devolve um erro ao agente que delegou** ("the delegation was stopped by a person"), que segue o turno em vez de morrer junto. A tarefa deixa de ser controlável só depois do desfecho gravado (`FinishOnDrop`, declarado antes do `CancelOnDrop`). Parar a tarefa de cima leva a síncrona junto (o `branch` e a queda do futuro). **Não dá para pausar**: nada olha um portão antes das chamadas ao modelo ali, então `is_pausable` é falso, `control_agent_task` recusa a pausa ("can only be stopped") e `AgentTaskDto.pausable` (ausente = falso) faz as duas telas oferecerem só "Stop" nela.
- **Pausar a delegação síncrona** (Sessão 158): o `run_recorded` agora entrega o `PauseGate` da tarefa ao trabalho (`FnOnce(PauseGate) -> Future`, e um portão nunca pausado quando não há registro), e os dois delegadores o passam ao turno com `Orchestrator::with_pause_gate`, que só põe o portão (o `with_parent_task` também o põe, a partir do `TaskLink`). O laço do orquestrador passou a ler o campo `pause_gate` em vez de `task_link`. `Runner::is_pausable` saiu: toda tarefa controlável é pausável, e `control_agent_task` só recusa a pausa de tarefa pendente ou já pausada. **Decisão**: a delegação síncrona ganha só o portão, **não** o vínculo de subtarefas (nada de aninhar sob ela), para não mudar o que ela pode fazer. A pausa vale antes da próxima chamada ao modelo (espera a chamada em andamento e a ferramenta). `AgentTaskDto.pausable` segue no protocolo e agora é verdadeiro para tudo que é controlável.
- **Políticas e limite de modelos pela porta estreita** (Sessão 159): `AgentOrgEdit` ganhou `SetDelegationModels { id, models }` e `SetModelPolicies { policies }`, tratados em `apply_org_edit` (a segunda sai cedo, como o `Remove`, porque troca `config.model_policies`). Reusam as checagens de um salvar: `check_policies`, `check_delegation_models` (o trim e a deduplicação, extraídos de `check_agents`) e `prune_delegation_models` (uma política removida sai do limite de todo agente). **Diferença do salvar**: um id desconhecido no limite é **recusado com o nome**, em vez de sumir calado. Sem `base_version`: é ler, mudar e gravar sob o lock dos salvamentos, então vale a última escrita no campo que toca. Só agentes do dono; membro já é recusado em `member_refusal`. O `settings` que vai aos clientes leves traz os ids de provedores e combos (o que uma política pode responder). A web e o desktop seguem editando pelo formulário de Configurações.
- **Threads como conversa filha** (Sessão 160, P125): uma thread é um `Conversation` com `parent: Option<ThreadParent { conversation_id, message_id }>`, fixado ao criar, e não um `thread_id` nas mensagens da mesma conversa. Por quê: tudo que o hub já faz por conversa (contexto de um arquivo só, trava de escrita, criptografia de membro, escopo de agente, projeto e pasta, `ChatResponse.conversationId`, renomear, apagar) continua valendo sem mudança; o desenho por mensagem pediria filtros em cada leitura. Regras: uma thread por mensagem e nenhuma dentro de outra; a filha herda projeto e pasta; apagar a pai apaga as filhas (varre a pasta); a lista de conversas **não** esconde as filhas no hub (o `ConversationsChanged` precisa achá-las), os clientes filtram `parent`; sem sync a fazer (o sync do cofre não toca em conversas). **Contexto**: `handle_agent_turn_in` põe antes do histórico da filha as últimas `THREAD_CONTEXT_MAX` (40) mensagens da pai que terminam na âncora; sem mudar o orquestrador. Uma conversa comum continua sem teto. **Protocolo**: `HistoryMessage.id` (o id estável que o arquivo já guardava e que o fio não levava), `ConversationSummary.parent` e `replies` (o contador sai da lista que o cliente já tem), `Chat.threadOf` (só lido ao criar, como `project_id`). **Segurança**: o id da conversa-pai vem do cliente e vira nome de arquivo, então é validado (`is_conversation_id`) antes de qualquer leitura, no hub e no bootstrap; sem isso um membro leria a conversa de outro por `../`. Recusado: thread de projeto de código (o contexto lá é a sessão do motor), thread de thread, mensagem inexistente. O agente da thread é o que o cliente manda (o da conversa aberta); o hub não o herda.
- **Atividade por nó e arrastar** (Sessão 159): a atividade é dobrada no cliente a partir de `ListAgentTasks` (`activityOf`: o que o agente recebeu por estado, tokens, quantas delegou, o último movimento), sem armazenamento nem mensagem nova; o limite é o do log (200 tarefas) e conversas diretas e recados não entram. O arrastar é HTML5 nativo no `.org-card` e usa `moveEdit`, que devolve o `setPosition` com o cargo que o agente já tem ou `null` se o alvo fecharia um círculo ou já é o superior; o hub continua sendo quem decide (`check_hierarchy`).
- **Fica**: CLI (`/jobs`), celular e extensão; pausar a delegação síncrona (pede um portão no turno dela).
- **Canal fixo por agente e mensagem iniciada pelo agente** (Sessões 165 e 167, P121): o canal de um agente é uma conversa comum de id `channel-<FNV-1a 64 bits do nome>`, calculado só no hub (`channel_id`) e pedido pelos clientes com `OpenAgentChannel`, porque o id do agente é texto livre e o de uma conversa é nome de arquivo, e assim Rust, TypeScript e Dart não repetem o hash; sem campo novo no arquivo da conversa, os clientes escondem o prefixo `channel-` da lista do Chat (como escondem as threads). Convive com as conversas soltas, as `A → B` e as `task-*`. A mensagem que o agente inicia é uma mensagem comum do assistente nesse canal (`message_user`, `outreach.rs`); quem autoriza é a tabela `[[outreach]]` do `config.toml` (por agente, com `forward` reservado para os canais externos), e não um campo novo do `AgentConfig`, para não repetir o alcance de uma flag em todas as telas e para o `forward` morar junto da autorização. Só uma pessoa edita a tabela; nenhuma tool a escreve. Limite de 12 mensagens por hora por agente, em memória. Também vale nas execuções sem ninguém olhando (tarefas e webhooks), que recebem a pasta das conversas. **Envio externo (Sessão 168)**: o `forward` do `[[outreach]]` deixa o texto (`agente: mensagem`) numa pasta `bot_outbox/` ao lado do `config.toml`, um arquivo por mensagem (sem lock: o hub escreve e o bot lê e apaga sem tocar no mesmo arquivo; expira em 24 h, 50 por canal), que o bot do canal envia aos **chats do dono** (listados e não mapeados para um membro). Só em execução sem ninguém olhando e do dono (`AgentExtras.forward_outreach`, ligado em `tasks.rs`); um turno que uma pessoa começou nunca encaminha, porque ela lê o canal e porque pode ser um membro, cujas mensagens não podem ir aos chats do dono. **Edição pela tela (Sessão 170)**: a tabela `[[outreach]]` é editada nas Configurações da web (hub) e do desktop (config local), num checkbox por agente com os canais externos; viaja em `HubSettingsDto.outreach`/`HubSettingsUpdate.outreach` (`None` = a tela não mexeu: as entradas ficam, seguem uma renomeação e somem com o agente apagado) e passa por `settings::check_outreach` (agente do dono que existe, sem repetição, canais `telegram`/`whatsapp`). Nenhuma tool escreve essa tabela. **Feed de atividade (Sessão 171)**: `activity.rs` monta a linha do tempo na hora do pedido (`ListActivity`), sem log próprio: eventos de tarefa a partir de `agent_tasks.jsonl` (`delegated`, `started`, `done`/`failed`/`cancelled`, com os carimbos que a tarefa já tem), recados entre agentes a partir das conversas `agents-…` (título "A → B": mensagem do usuário = `note`, do assistente = `reply`) e mensagens que o agente começou a partir das `channel-…` (assistente que não segue uma mensagem da pessoa = `messaged_user`). Lê só os arquivos com esses prefixos, nos dois diretórios do dono (aparelho e tarefas agendadas); no mesmo milissegundo a etapa mais adiantada vem primeiro. Só o dono vê. **Sessão 174**: as execuções de tarefa agendada e de webhook também saem do que já existe (as conversas `task-…`/`task-hook-…`, um evento por prompt de execução seguido da resposta; `(could not run:` = falhou); o único registro escrito para o feed é `agent_changes.jsonl`, ao lado do `config.toml`, com os agentes que o `manage_agents` criou ou removeu (o que uma pessoa faz pela tela não entra), porque o `config.toml` só guarda os agentes que existem. **Clientes (Sessão 172)**: web (aba "Atividade"), desktop (botão "Activity"; no hub pelo `ListActivity`, neste computador pelo comando Tauri `list_activity`, que lê os mesmos arquivos locais, sem canais), extensão (sub-aba "Atividade") e celular (aba "Activity"); cada um tem um módulo puro com a frase, o filtro, os grupos por dia e o destino do clique, e atualiza a cada 3 s. **Notificação do sistema (Sessão 173)**: nos quatro clientes, quando chega um `ConversationsChanged` de um canal não lido que não está em frente e cuja última mensagem é do agente. Web: `Notification` do navegador; celular: notificação local; desktop: `tauri-plugin-notification` (só no hub, sem clique, porque o plugin não tem clique no computador); extensão: `chrome.notifications` pelo background, com o id do canal e o ícone do app, e o clique põe o painel no canal e tenta abri-lo.
