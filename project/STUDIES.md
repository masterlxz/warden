# Estudos

> Estudos de fora (outros projetos, artigos) que alimentam decisões do Warden. Cada um diz o que foi lido, o que os
> dados mostram, como o Warden está hoje e o que faz sentido trazer. A decisão em si vai em `ARCHITECTURE.md` e o
> trabalho em `PENDING.md`.

---

## Hermes Agent e o aprendizado dos agentes (P104, Sessão 115, 2026-09-30)

**O que foi lido**: a documentação oficial do Hermes Agent (Nous Research; MIT, lançado em fevereiro de 2026), o README,
issues do GitHub, um artigo crítico da memória e um artigo de pesquisa sobre segurança de skills. **Não foi lido o
código**; os números de desempenho abaixo vêm de um issue e de um artigo de terceiros, não de medição nossa.

### Como o Hermes aprende

| Camada | O que é | Detalhe |
|---|---|---|
| Memória no prompt | `MEMORY.md` (2.200 caracteres) e `USER.md` (1.375) em `~/.hermes/memories/` | Tool `memory` com `add`, `replace` (por trecho) e `remove`, sem `read`. Se a escrita estoura o limite, dá erro e o agente tem de consolidar no mesmo turno; o uso aparece em percentual. O conteúdo entra **congelado** no começo da sessão (separado por `§`), para preservar o cache do prefixo: o que muda na sessão só aparece na próxima. |
| Busca em sessões passadas | SQLite (`state.db`) com FTS5 | Tool `session_search`, com resumo por LLM; só roda quando o agente chama. Não consome orçamento fixo de tokens. |
| Skills | `~/.hermes/skills/<categoria>/<nome>/SKILL.md` (+ `references/`, `templates/`, `scripts/`), padrão agentskills.io | Três níveis de carregamento: lista (~3k tokens), conteúdo, arquivo de apoio. Tool `skill_manage` com `create`, `patch` (trocar um trecho, o preferido, gasta menos tokens), `delete`, `write_file`, `remove_file`. O prompt manda registrar uma skill quando acha um fluxo de vários passos, resolve um erro com contorno ou leva uma correção do usuário; em geral depois de tarefas com 5 ou mais chamadas de tool. As lições devem ser regras generalizadas, não relato do incidente. |
| Ativação condicional | Metadados `fallback_for_toolsets`, `requires_toolsets`, `requires_tools` | A skill aparece ou some conforme as tools disponíveis. |
| Revisão em segundo plano | Um "fork" do agente com tools de memória, de skills e de leitura de arquivo | Dispara **por relógio**: a cada `memory.nudge_interval` turnos do usuário e a cada `skills.creation_nudge_interval` iterações de tool (0 desliga a criação de skills). |
| Provedores externos de memória | Sete plugins (Honcho, Mem0, Holographic, RetainDB, ByteRover, OpenViking, Supermemory) | Rodam ao lado da memória própria: grafos de conhecimento, busca semântica, modelo do usuário entre sessões. |
| Salvaguardas | `skills.write_approval: true` deixa toda escrita de skill em `~/.hermes/pending/skills/` até aprovação; skills vindas de hub passam por um scanner (exfiltração, injeção de prompt, comandos destrutivos, supply chain) | O scanner é consultivo e não bloqueia. Há níveis de confiança (builtin, trusted, community), e skills de projeto exigem `hermes skills trust`. |

### O que os dados dizem

- **A revisão por relógio gera lixo, sobretudo na memória.** Issue 128884 do Hermes (medido com um modelo Nemotron): 90% das
  revisões em janelas sem instrução do usuário ainda escreviam algo; 20 de 22 escritas ruins ou duplicadas eram de
  memória (reescrever uma preferência, registrar o estado de uma tarefa, inventário de arquivos), e 28 de 35 lições reais
  eram de skills, descobertas pelo próprio trabalho do agente. Sessões curtas nunca disparam a revisão. A proposta do
  issue: um gancho por turno que decide se houve aprendizado e dispara ou suprime a revisão, separando duas fontes
  (memória: só correção ou revelação do usuário; skill: correção ou algo que o agente descobriu). O detector de referência
  chegou a 0,90 de AUC por cerca de US$ 0,002 em 57 turnos.
- **Limites da memória do Hermes** (artigo da Vectorize): o teto de ~1.300 tokens força compressão e perda de detalhe; a
  busca FTS5 é só por palavras e falha com paráfrase e relações; não há resolução de entidades ("Alice" e "minha colega
  Alice"); o que não foi marcado antes da compressão do contexto se perde; e a curadoria depende do julgamento do agente,
  então sessões curtas podem não guardar nada.
- **Skills escritas pelo agente podem evoluir para inseguras** (artigo "Practice Makes Unsafe", arXiv 2608.12851,
  SkillMisevo-Bench): as 21 configurações de agente testadas produziram artefatos inseguros, 15 levaram a dano numa sessão
  nova, e com três tarefas maliciosas a taxa de sucesso do ataque que persiste de uma sessão para a outra subiu de 16,0%
  para 35,3%. O motivo: a evolução otimiza a conclusão da tarefa, não a segurança, e um sucesso inseguro vira política
  permanente. O SafeEvolve (repara o conteúdo e governa a reutilização) baixou em 26,7 pontos a recuperação de skill
  insegura e em 17,3 pontos o dano em sessão nova, com 0,4 ponto de perda de utilidade. A conclusão dos autores: a segurança
  da adaptação persistente tem de controlar o que persiste **e** o que as execuções futuras podem acessar.

### Como o Warden está hoje

- **Memória**: notas no vault, achadas pela busca (texto e semântica) com as 8 melhores a cada turno. **Depois do P94 (Sessão
  115) não há mais memória fixa**: o permanente vai na persona do agente.
- **Skills** (P16/P72): catálogo no prompt (nome e descrição), `use_skill`, `read_skill_file`, escopo por agente e o gerador
  de rascunho (`skill_gen`), que só devolve um rascunho para a pessoa revisar.
- **`manage_skill`** só tem `create` e `update` (troca a skill inteira), e a descrição manda usar **somente quando o usuário
  pede**. Não há aprendizado automático nem revisão em segundo plano.
- **O agente não busca conversas antigas**: não existe tool de histórico (as conversas ficam em arquivos por pessoa, cifrados
  para membros).
- **Já existe e ajuda**: aprovações (um "sim" antes de uma tool), limites de gasto por pessoa, agentes com escopo, vault
  cifrado por membro e a regra de que o root não lê o vault dos outros (P84).

### O que faz sentido trazer (ordem de valor e risco)

1. **`search_history`**: o agente busca nas conversas da própria pessoa (texto, e semântica se couber). É o papel do
   `session_search`. Para membros as conversas são cifradas: a busca decifra em memória e só para aquela pessoa.
2. **Detector pós-turno no lugar do relógio**: um modelo barato decide "houve aprendizado?" (correção ou revelação do
   usuário; descoberta do agente ao trabalhar) e só então dispara a revisão.
3. **A revisão propõe, a pessoa aprova**: propostas de skill (nova ou alteração) ficam numa lista pendente, com o trecho da
   conversa de origem, e nada vale antes do aceite. Cobre o risco do artigo de segurança.
4. **`manage_skill` ganha `patch`** (trocar um trecho), gastando menos tokens e sem reescrever a skill inteira.
5. **Foco em skills, não em memória**: os dados do Hermes mostram que a memória automática é a mais ruidosa. Combina com o
   P94 (acabar com os arquivos fixos): aprender vira skill e nota, não um perfil sempre injetado.

### Regras que só o Warden precisa (multiusuário)

- O que uma pessoa ensina **não vaza**: uma skill aprendida numa conversa da Ana nasce restrita à Ana (escopo de pessoa) e
  nunca vira skill do workspace ou do root sem o root aprovar.
- A revisão conta no **limite de gasto** da pessoa e usa um modelo barato configurável (as combos de modelos já existem).
- Uma skill aprendida nasce **pendente**, com origem registrada (qual conversa, quando, por qual agente), e o conteúdo passa
  pelas mesmas conferências de uma skill escrita à mão (nome, tamanho, escopo).

### O que foi feito (Sessão 115, fatia 1)

`search_history`, o detector em dois estágios e as skills sugeridas pendentes de aceite, com `[learning]` desligado por padrão
(detalhes em `ARCHITECTURE.md`). Resultado com um modelo real barato: uma correção da pessoa gerou uma skill boa e uma
conversa banal não gerou nada. Falta o que está no P115 do `PENDING.md`.

### Perguntas em aberto

- O quanto é automático: sempre pedir aceite, ou aceitar sozinho o que é de baixo risco (por exemplo, só texto, sem
  scripts)?
- Onde ficam as propostas pendentes (uma pasta do vault, ou estado do hub) e como aparecem (web, desktop, celular).
- Qual modelo faz o detector e a revisão, e se um membro pode escolher o dele.
- Se a busca de histórico entra na semântica ou começa só com texto.

### Fontes

- Documentação de memória: https://hermes-agent.nousresearch.com/docs/user-guide/features/memory
- Documentação de skills: https://hermes-agent.nousresearch.com/docs/user-guide/features/skills
- Repositório: https://github.com/nousresearch/hermes-agent
- Issue 128884 (revisão por sinal de aprendizado): https://github.com/NousResearch/hermes-agent/issues/128884
- Artigo crítico da memória: https://vectorize.io/articles/hermes-agent-memory-explained
- Practice Makes Unsafe: Skill Misevolution in Self-Improving LLM Agents: https://arxiv.org/abs/2608.12851
