# Visão: o Warden como um sistema operacional de agentes

> **Origem**: documento trazido pronto pelo usuário em 2026-10-04 (`ai-hub-agents-spec.md`, na raiz do repositório),
> removido da raiz e incorporado aqui, **na íntegra** (Parte 1). A Parte 2 compara a visão com o que o Warden já tem
> e a Parte 3 lista as decisões em aberto. As pendências derivadas estão no `PENDING.md` (P120 a P124) e o resumo, no
> `ROADMAP.md` ("Sistema operacional de agentes").
>
> **Estado**: só registro. Nada disto foi levado a `/plan`, e o próprio documento diz que as decisões vêm depois
> (seção 50). O nome do produto no texto original é "AI Hub" e o operador de exemplo é o Fabio; o projeto se chama
> Warden.

| Parte | O que tem |
|---|---|
| **1. A visão** | O documento do usuário, como foi escrito: 50 seções (Chat, Agents, Code, hierarquia, autoridade, autonomia, hospedagem). |
| **2. O Warden hoje × a visão** | O que já existe, o que existe pela metade e o que não existe, seção por seção. |
| **3. Decisões em aberto** | Tensões com o que já foi decidido e uma ordem sugerida (não decidida). |

---

# Parte 1 — A visão (documento original)

# AI Hub — Especificação Conceitual do Sistema de Chat, Agents e Code

> Documento de visão de produto e comportamento.  
> Não define implementação, stack ou arquitetura interna de código.

---

## 1. Visão geral

O projeto é um **hub pessoal e empresarial de IA**, com foco em reunir diferentes modelos, ferramentas e agentes em uma única experiência.

A ideia central é permitir que o usuário:

- converse com uma IA de forma tradicional;
- crie e utilize agentes especializados;
- faça agentes trabalharem uns com os outros;
- crie hierarquias de agentes;
- delegue tarefas entre agentes;
- utilize agentes para tarefas empresariais, pessoais e técnicas;
- utilize um ambiente especializado de programação;
- escolha entre modelos e agentes diferentes conforme a tarefa;
- hospede o sistema por conta própria ou utilize uma hospedagem fornecida pelo projeto.

O sistema deve funcionar tanto como um **assistente pessoal completo** quanto como uma plataforma de **automação e organização de trabalho com múltiplos agentes**.

---

# 2. Conceito central: agentes globais

Os agentes são entidades globais dentro do sistema.

Um agente não pertence exclusivamente a uma conversa, ao modo Chat ou ao modo Code.

O mesmo agente pode ser utilizado em diferentes contextos:

- Chat;
- Agents;
- Code;
- tarefas automáticas;
- outros módulos futuros.

### Exemplo

Um agente chamado `Programming Manager` pode:

- conversar diretamente com o usuário no Chat;
- aparecer como uma conversa própria no modo Agents;
- ser selecionado dentro do Code;
- receber tarefas de outro agente;
- delegar tarefas para agentes subordinados.

Isso cria uma espécie de **organização virtual de IAs**.

---

# 3. Três modos principais

A interface deve possuir três grandes modos/módulos:

1. **Chat**
2. **Agents**
3. **Code**

A troca entre eles deve ser extremamente clara e rápida.

---

# 4. Modo Chat

O Chat é a experiência tradicional de assistente de IA.

A experiência deve ser próxima de ChatGPT/Gemini:

- múltiplas conversas;
- histórico de conversas;
- cada conversa pode tratar de um assunto diferente;
- uma conversa pode existir sem agente;
- uma conversa pode utilizar um agente;
- o usuário pode trocar de agente/modelo quando fizer sentido;
- o usuário não precisa criar um agente para utilizar o sistema.

### Modelo mental

O Chat responde à pergunta:

> "Quero conversar ou resolver alguma coisa."

Uma conversa pode ser:

```text
Chat
├── Projeto pessoal
├── Estudos
├── Ideia de empresa
├── Viagem
└── Desenvolvimento de aplicativo
```

E qualquer uma dessas conversas pode utilizar:

```text
Sem agente
        ↓
Modelo escolhido diretamente

OU

Agente
        ↓
Agente decide como trabalhar
```

---

# 5. Chat + agentes

Um agente pode ser selecionado dentro de uma conversa normal.

Por exemplo:

```text
Conversa: "Criar meu aplicativo"

Usuário
   ↓
Agente Principal
   ↓
Programming Manager
   ↓
Agentes especializados
```

O usuário não precisa necessariamente abrir o modo Code.

Ele pode simplesmente dizer:

> "Preciso criar uma aplicação simples. Gere a especificação e implemente."

O agente de Chat pode:

1. entender o pedido;
2. gerar a especificação;
3. identificar que a tarefa exige programação;
4. enviar a especificação para o agente responsável por programação;
5. acompanhar o trabalho;
6. receber o resultado;
7. apresentar o resultado ao usuário.

---

# 6. Modo Agents

O modo Agents é diferente do Chat.

Ele representa os agentes como **entidades persistentes**, com suas próprias conversas.

O conceito é semelhante ao funcionamento de um WhatsApp:

```text
Agents

┌──────────────────────────────┐
│ 🤖 Principal                 │
│ Última mensagem...           │
├──────────────────────────────┤
│ 💻 Programming Manager       │
│ Implementação concluída...   │
├──────────────────────────────┤
│ 🔒 Security Agent            │
│ Encontrei uma vulnerabilidade │
├──────────────────────────────┤
│ 🐧 Linux Manager             │
│ Servidor atualizado...       │
└──────────────────────────────┘
```

Cada agente possui uma conversa principal persistente.

---

# 7. Conversa de agente = canal persistente

No modo Agents, a conversa não deve ser simplesmente uma sequência de prompts do usuário.

O agente pode agir autonomamente.

Exemplo:

```text
👤 Fabio
Preciso monitorar meu servidor.

🤖 Server Manager
Vou configurar o monitoramento.

🤖 Server Monitor
CPU está em 73%.

🤖 Server Monitor
Memória está em 81%.

🤖 Server Manager
O servidor continua saudável.

🤖 Server Monitor
Atualização concluída.

🤖 Server Manager
Tudo certo.
```

O usuário pode simplesmente observar e participar.

O agente pode enviar mensagens sem que o usuário tenha enviado um prompt imediatamente antes.

---

# 8. Agentes podem iniciar mensagens

Agentes devem possuir capacidade de iniciar comunicação quando autorizado.

Casos de uso:

- notificações;
- monitoramento;
- conclusão de tarefas;
- alertas;
- relatórios;
- lembretes;
- acompanhamento de processos;
- perguntas para o usuário;
- atualizações periódicas.

Exemplo:

> "Me informe o status do servidor a cada 30 minutos."

O agente pode então enviar:

```text
08:00 — Servidor saudável.
08:30 — Servidor saudável.
09:00 — CPU acima do normal.
09:30 — CPU normalizada.
```

Isso transforma o agente em algo mais próximo de um **funcionário digital persistente** do que simplesmente um chatbot.

---

# 9. Comunicação entre agentes

Esse é um dos princípios mais importantes do sistema.

Agentes devem poder conversar entre si.

A comunicação deve ser observável e legível pelo usuário.

Exemplo:

```text
👤 Fabio
Crie uma aplicação de controle financeiro.

🤖 Personal Assistant
Vou preparar a especificação.

🤖 Personal Assistant → Programming Manager
Preciso que você implemente a aplicação descrita nesta spec.

🤖 Programming Manager
Entendido. Vou dividir o trabalho.

🤖 Programming Manager → Backend Agent
Implemente a API.

🤖 Programming Manager → Frontend Agent
Implemente a interface.

🤖 Backend Agent
API concluída.

🤖 Frontend Agent
Interface concluída.

🤖 Programming Manager
Integração finalizada.

🤖 Personal Assistant
O projeto foi concluído.
```

A comunicação entre agentes deve parecer uma conversa real entre membros de uma equipe.

---

# 10. Visibilidade da comunicação

O sistema deve permitir visualizar:

- quem enviou a mensagem;
- para quem a mensagem foi enviada;
- qual agente respondeu;
- qual tarefa foi criada;
- qual agente delegou a tarefa;
- qual agente concluiu;
- quais agentes estão trabalhando.

A interface pode utilizar visualmente:

```text
Programming Manager
       │
       ├──→ Backend Agent
       │
       ├──→ Frontend Agent
       │
       └──→ Security Agent
```

Além disso, cada mensagem pode indicar:

```text
Programming Manager → Security Agent
```

Isso deve tornar o funcionamento do sistema compreensível sem esconder a colaboração interna.

---

# 11. Modo Code

O modo Code é o ambiente especializado para desenvolvimento.

Ele utiliza o núcleo de programação/harness já previsto no projeto, incluindo a possibilidade de integração com ferramentas como OpenCode.

O Code deve parecer mais próximo de um ambiente de desenvolvimento do que de um simples chatbot.

---

# 12. Code como uma experiência híbrida

O usuário pode trabalhar de duas maneiras:

### Sem agente

```text
Code
↓
Modelo
↓
Ferramentas
↓
Execução
```

Isso é semelhante a utilizar um coding assistant tradicional.

### Com agente

```text
Code
↓
Programming Manager
↓
Subagentes
↓
Ferramentas / modelos
↓
Execução
```

O usuário escolhe o nível de autonomia.

---

# 13. Um agente especializado pode coordenar o Code

O principal agente de programação pode funcionar como um **Programming Manager**.

Ele não precisa executar tudo diretamente.

Ele pode:

- interpretar a tarefa;
- analisar o projeto;
- criar um plano;
- dividir tarefas;
- escolher agentes;
- escolher modelos;
- delegar;
- acompanhar;
- revisar;
- solicitar correções;
- integrar resultados;
- finalizar o projeto.

Exemplo:

```text
Programming Manager

        │
        ├── Architecture Agent
        │
        ├── Backend Agent
        │
        ├── Frontend Agent
        │
        ├── Database Agent
        │
        ├── Testing Agent
        │
        └── Security Agent
```

---

# 14. Agentes temporários no Code

Nem todo agente precisa ser permanente.

O Programming Manager pode criar agentes temporários para tarefas específicas.

Exemplo:

```text
Programming Manager
       ↓
"Preciso analisar este bug específico."
       ↓
Bug Analysis Agent
       ↓
Analisa
       ↓
Resolve
       ↓
Agente temporário encerrado
```

Esses agentes não precisam aparecer como agentes permanentes do usuário.

Eles são **workers temporários**.

Isso evita poluir a lista global de agentes.

---

# 15. Agentes globais vs. subagentes temporários

Existem dois conceitos:

### Agentes persistentes

São agentes criados pelo usuário ou pela organização.

Exemplos:

- Personal Assistant;
- Programming Manager;
- Finance Manager;
- Security Manager;
- Linux Manager.

Eles aparecem na tela Agents.

### Agentes temporários

São criados para executar tarefas específicas.

Exemplos:

- Debug Agent;
- Test Generator;
- API Research Agent;
- Refactoring Agent.

Eles podem existir somente durante a execução de uma tarefa.

---

# 16. O mesmo agente pode trabalhar em qualquer módulo

Um agente não deve estar preso ao módulo onde foi criado.

Exemplo:

```text
Programming Manager
```

pode ser utilizado em:

```text
Chat
Agents
Code
Automations
```

Isso é essencial para criar um ecossistema realmente integrado.

---

# 17. Exemplo completo: criar uma aplicação pelo Chat

Usuário está no Chat:

> "Quero criar um aplicativo simples para controlar meus investimentos."

O fluxo pode ser:

```text
Chat Agent
    │
    ├── entende o pedido
    │
    ├── cria a especificação
    │
    └── envia para Programming Manager
                    │
                    ├── Backend Agent
                    ├── Frontend Agent
                    ├── Database Agent
                    └── Testing Agent
```

O usuário pode escolher:

### Opção A — acompanhar no Chat

O Chat continua mostrando atualizações.

### Opção B — abrir o Code

O usuário entra no módulo Code e vê o trabalho acontecendo.

### Opção C — simplesmente esperar

O usuário deixa o agente trabalhando e recebe uma mensagem quando terminar.

---

# 18. Hierarquia de agentes

O sistema deve permitir criar uma hierarquia real entre agentes.

Não deve ser apenas uma lista de agentes.

A estrutura pode ser:

```text
                    👑 Principal Agent
                           │
              ┌────────────┼────────────┐
              ↓            ↓            ↓
         Tech Manager  Finance Manager  Personal Manager
              │
       ┌──────┼───────┐
       ↓      ↓       ↓
    Backend Frontend Security
```

Cada agente possui:

- cargo;
- nível hierárquico;
- superior;
- subordinados;
- permissões;
- responsabilidades;
- modelos permitidos;
- ferramentas permitidas;
- capacidade de delegação.

---

# 19. Hierarquia visual

A criação e edição da hierarquia deve ser **visual**.

Não deve depender apenas de escrever configurações.

O usuário deve conseguir visualizar uma árvore:

```text
                 Principal
                     │
             ┌───────┴───────┐
             │               │
         Tech Manager    Business Manager
             │
       ┌─────┼─────┐
       │     │     │
     Backend UI  Security
```

E poder:

- criar agente;
- mover agente;
- mudar superior;
- criar subordinado;
- alterar cargo;
- remover agente;
- visualizar responsabilidades;
- abrir conversa;
- visualizar tarefas;
- visualizar atividade.

---

# 20. Hierarquia modificável pelos próprios agentes

A hierarquia não precisa ser completamente estática.

Agentes autorizados podem administrar outros agentes.

Exemplo:

```text
Principal Agent
       ↓
cria
       ↓
Programming Manager
       ↓
cria
       ↓
Backend Agent
```

O Programming Manager pode posteriormente:

```text
Backend Agent
     ↓
está com desempenho ruim
     ↓
Programming Manager
     ├── remove Backend Agent
     └── cria Backend Agent 2
```

Isso permite que a organização virtual evolua.

---

# 21. Regra fundamental de autoridade

Um agente não pode alterar a própria posição hierárquica livremente.

A regra deve ser:

> **Um agente só pode modificar entidades que estejam dentro do seu escopo de autoridade.**

Exemplo:

```text
Principal
   │
   ├── Manager A
   │      ├── Worker A1
   │      └── Worker A2
   │
   └── Manager B
          └── Worker B1
```

Manager A pode:

- criar A3;
- remover A1;
- substituir A2;
- delegar tarefas aos seus subordinados.

Manager A não pode:

- remover Manager B;
- tornar-se Principal;
- alterar as permissões do Principal;
- alterar sua própria posição para ficar acima do Principal.

---

# 22. Escopo de autoridade

Cada agente deve possuir permissões explícitas.

Exemplos:

```text
Pode criar agentes
Pode remover agentes
Pode editar agentes
Pode delegar tarefas
Pode escolher modelos
Pode escolher ferramentas
Pode criar agentes temporários
Pode executar código
Pode acessar arquivos
Pode acessar serviços externos
Pode enviar mensagens ao usuário
Pode iniciar tarefas
```

Isso permite diferentes níveis de autonomia.

---

# 23. Cargos

O sistema deve tratar agentes como membros de uma organização.

Exemplos:

- Principal;
- Director;
- Manager;
- Specialist;
- Worker;
- Reviewer;
- Assistant;
- Coordinator.

O cargo não precisa ser apenas decorativo.

Ele pode representar:

- posição na hierarquia;
- responsabilidades;
- permissões;
- capacidade de delegação.

---

# 24. Agentes podem criar agentes

Um agente autorizado pode criar outro agente.

Exemplo:

```text
Programming Manager
       ↓
"Preciso de um especialista em PostgreSQL."
       ↓
cria
       ↓
PostgreSQL Specialist
```

O novo agente recebe:

- objetivo;
- cargo;
- superior;
- permissões;
- contexto;
- ferramentas;
- modelo padrão;
- limites de atuação.

---

# 25. Gerenciamento de agentes por agentes

Agentes gerentes devem poder:

- contratar/criar agentes;
- dispensar/remover agentes;
- substituir agentes;
- delegar tarefas;
- revisar resultados;
- reorganizar subordinados;
- criar agentes temporários;
- alterar responsabilidades dentro do seu escopo.

Isso transforma a hierarquia em uma **organização dinâmica**.

---

# 26. Escolha de modelo por tarefa

O agente gerente não precisa utilizar sempre o mesmo modelo.

Ele pode escolher:

```text
Tarefa
  ↓
qual modelo é adequado?
  ↓
modelo rápido
modelo barato
modelo de raciocínio
modelo de código
modelo multimodal
```

Exemplo:

```text
Programming Manager

Arquitetura → modelo de raciocínio
Código       → modelo especializado em programação
Testes       → modelo barato
Review       → modelo de alta qualidade
Documentação → modelo rápido
```

O agente pode decidir isso automaticamente de acordo com as políticas configuradas.

---

# 27. Agentes + modelos são conceitos diferentes

É importante separar:

**Agente**

> Quem decide o que fazer e como trabalhar.

**Modelo**

> O motor de IA usado pelo agente para raciocinar/gerar respostas.

Assim:

```text
Programming Manager
       ↓
escolhe
       ↓
Modelo A
Modelo B
Modelo C
```

Um agente pode trocar de modelo conforme a tarefa.

---

# 28. Agentes + ferramentas

Da mesma forma, ferramentas devem ser separadas dos agentes.

Um agente pode ter acesso a:

- terminal;
- arquivos;
- Git;
- navegador;
- banco de dados;
- APIs;
- servidores;
- Docker;
- ferramentas de desenvolvimento;
- serviços externos;
- outros agentes.

Isso permite criar agentes especializados sem precisar criar um sistema completamente separado para cada função.

---

# 29. Sistema de delegação

A delegação deve ser uma operação fundamental.

Exemplo:

```text
Task
│
├── responsável: Programming Manager
├── prioridade: alta
├── objetivo: criar API
└── subtarefas:
      ├── Backend Agent
      ├── Database Agent
      └── Testing Agent
```

Cada agente pode receber uma tarefa e produzir:

- resultado;
- status;
- arquivos;
- mensagens;
- subtarefas;
- solicitações de revisão.

---

# 30. Estado das tarefas

O usuário deve conseguir visualizar:

```text
○ Pendente
◐ Em andamento
◉ Aguardando agente
✓ Concluído
⚠ Falhou
⏸ Pausado
```

E também:

```text
Programming Manager
████████████░░░ 80%

Backend       ✓
Frontend      ✓
Database      ◐
Testing       ○
Security      ○
```

---

# 31. Visão de organização

Além das conversas, o sistema deve possuir uma visão organizacional.

Exemplo:

```text
ORGANIZATION

Principal Agent
│
├── Programming Manager
│   ├── Backend
│   ├── Frontend
│   ├── Database
│   └── Security
│
├── Finance Manager
│   ├── Research
│   └── Analysis
│
└── Personal Manager
    ├── Calendar
    └── Reminders
```

Cada nó pode ser aberto para:

- conversa;
- tarefas;
- configurações;
- permissões;
- histórico;
- atividade.

---

# 32. Activity Feed

O sistema deve possuir uma visão de atividade.

Exemplo:

```text
19:32  Personal Assistant criou uma tarefa.
19:33  Programming Manager recebeu a tarefa.
19:34  Programming Manager criou Backend Agent.
19:35  Backend Agent iniciou implementação.
19:41  Backend Agent concluiu API.
19:42  Security Agent iniciou revisão.
19:45  Security Agent encontrou um problema.
19:47  Programming Manager delegou correção.
```

Isso permite acompanhar organizações complexas sem precisar abrir cada conversa.

---

# 33. Conversas como primeira classe

Apesar da existência da hierarquia, as conversas continuam sendo importantes.

Cada agente persistente deve possuir sua própria conversa principal.

Isso mantém a experiência simples:

```text
Agents
│
├── Principal
├── Programming Manager
├── Backend Agent
├── Security Agent
└── Finance Manager
```

Ao abrir um agente:

```text
┌─────────────────────────────────┐
│ Programming Manager             │
├─────────────────────────────────┤
│                                 │
│ 👤 Fabio                         │
│ ...                              │
│                                 │
│ 🤖 Programming Manager           │
│ ...                              │
│                                 │
│ 🤖 Security Agent                │
│ → revisão concluída              │
│                                 │
└─────────────────────────────────┘
```

---

# 34. Comunicação direta entre agentes

Além da hierarquia, agentes podem conversar diretamente quando permitido.

Exemplo:

```text
Backend Agent
      ↓
Security Agent

"Pode revisar minha API?"
```

O Security Agent responde:

```text
"Encontrei 2 problemas."
```

Essa comunicação deve ser registrada.

---

# 35. Comunicação hierárquica vs. comunicação lateral

Devem existir dois conceitos:

### Hierárquica

```text
Manager → Worker
```

Usada para:

- delegação;
- supervisão;
- criação;
- remoção;
- revisão.

### Lateral

```text
Worker A ↔ Worker B
```

Usada para:

- colaboração;
- consultas;
- revisão;
- troca de informações.

As permissões devem determinar quando a comunicação lateral é permitida.

---

# 36. Usuário como membro da organização

O usuário não precisa ser apenas um "cliente" conversando com uma IA.

Ele pode ser considerado o operador da organização.

Exemplo:

```text
                 Fabio
                   │
            Principal Agent
                   │
          ┌────────┴────────┐
          │                 │
    Programming         Business
      Manager             Manager
```

Isso permite construir uma verdadeira **empresa digital assistida por IA**.

---

# 37. Autonomia configurável

Cada agente deve possuir um nível de autonomia.

Exemplo:

```text
Autonomia

1 — Somente responder
2 — Sugerir ações
3 — Executar após aprovação
4 — Executar autonomamente
5 — Gerenciar subordinados autonomamente
```

O usuário pode limitar agentes críticos.

---

# 38. Aprovação humana

Algumas ações podem exigir aprovação do usuário.

Exemplos:

- apagar dados;
- gastar dinheiro;
- alterar infraestrutura crítica;
- enviar mensagens externas;
- publicar código;
- alterar configurações importantes;
- criar agentes com permissões elevadas.

Fluxo:

```text
Agent
 ↓
Solicita ação
 ↓
Usuário aprova
 ↓
Execução
```

---

# 39. Princípio de segurança da hierarquia

A hierarquia deve funcionar como um sistema de autoridade.

Um agente não ganha autoridade simplesmente porque "pediu".

A autoridade deve ser derivada de:

```text
posição
+
permissões
+
escopo
+
aprovação
```

O sistema deve impedir que um agente utilize outro agente para contornar suas próprias restrições.

---

# 40. Experiência desejada

A experiência geral deve parecer uma mistura de:

- ChatGPT/Gemini para conversa;
- WhatsApp para comunicação persistente com agentes;
- Slack para organização;
- ambiente de coding agent para desenvolvimento;
- organograma para hierarquia;
- sistema operacional de agentes para automação.

Mas tudo deve existir dentro de **um único ecossistema**.

---

# 41. Princípio: simples para começar, poderoso para crescer

O sistema não deve exigir que o usuário monte uma empresa de agentes antes de conseguir usar a IA.

Experiência inicial:

```text
Abrir
↓
Chat
↓
Conversar
```

Depois:

```text
Criar agente
↓
Especializar
↓
Delegar
↓
Criar hierarquia
↓
Automatizar
```

O poder deve aparecer conforme a necessidade.

---

# 42. Exemplo de evolução do usuário

### Nível 1

```text
Chat
└── uma conversa normal
```

### Nível 2

```text
Chat
├── conversa pessoal
└── conversa de programação
```

### Nível 3

```text
Agents
├── Personal Assistant
└── Programming Manager
```

### Nível 4

```text
Programming Manager
├── Backend
├── Frontend
└── Security
```

### Nível 5

```text
Principal
├── Programming Manager
│   ├── Backend
│   ├── Frontend
│   ├── Security
│   └── Testing
│
├── Finance Manager
│   ├── Research
│   └── Analysis
│
└── Personal Manager
    ├── Calendar
    └── Automation
```

---

# 43. Open Source + hospedagem

O projeto deve manter uma filosofia open source.

Possibilidades:

### Self-hosted

O usuário hospeda tudo por conta própria.

### Hosted

O projeto oferece infraestrutura hospedada.

O mesmo conceito funcional deve existir nos dois modelos.

O produto pode possuir planos pagos principalmente para:

- hospedagem;
- infraestrutura;
- modelos;
- armazenamento;
- execução;
- ferramentas premium;
- automações;
- recursos empresariais.

A filosofia central continua sendo:

> O usuário pode possuir e controlar sua própria instalação.

---

# 44. Ecossistema de modelos

O hub deve ser agnóstico em relação aos modelos.

O usuário deve poder conectar diferentes provedores/modelos.

Exemplo conceitual:

```text
Model Hub
├── Provider A
├── Provider B
├── Provider C
├── Local Models
└── Custom Endpoint
```

Os agentes utilizam esses modelos conforme suas configurações e políticas.

---

# 45. Ecossistema de ferramentas

Da mesma forma:

```text
Tool Hub
├── Git
├── Terminal
├── Browser
├── Files
├── Database
├── Docker
├── APIs
└── Custom Tools
```

Agentes recebem acesso apenas ao que precisam.

---

# 46. Princípio de interoperabilidade

Um agente criado para uma finalidade não deve ficar preso àquela finalidade.

Exemplo:

```text
Programming Manager
```

pode receber uma tarefa pelo Chat, trabalhar no Code e reportar o resultado no Agents.

Isso cria um fluxo contínuo:

```text
Chat
 ↓
Agent
 ↓
Code
 ↓
Agent
 ↓
Chat
```

Sem o usuário precisar copiar e colar informações manualmente.

---

# 47. Fluxo completo de exemplo

Usuário:

> "Quero criar um sistema web para controlar meus investimentos."

### Chat

```text
Personal Assistant
↓
Entende a ideia
↓
Gera spec
```

### Delegação

```text
Personal Assistant
↓
Programming Manager
```

### Planejamento

```text
Programming Manager
↓
define tarefas
```

### Execução

```text
Backend Agent
Frontend Agent
Database Agent
Security Agent
Testing Agent
```

### Comunicação

Os agentes conversam entre si conforme necessário.

### Code

O usuário pode abrir o Code e acompanhar:

```text
Programming Manager
├── Backend       ✓
├── Frontend      ◐
├── Database      ✓
├── Security      ○
└── Testing       ○
```

### Finalização

```text
Programming Manager
↓
Projeto concluído
↓
Personal Assistant
↓
Usuário
```

---

# 48. Princípios fundamentais do produto

1. **Agentes são globais.**
2. **Conversas são independentes dos agentes.**
3. **O mesmo agente pode atuar em vários módulos.**
4. **Chat é simples e livre.**
5. **Agents é persistente e orientado a comunicação.**
6. **Code é especializado em desenvolvimento.**
7. **Agentes podem conversar entre si.**
8. **Agentes podem agir sem prompt imediato quando autorizados.**
9. **Agentes podem criar agentes quando possuem autoridade.**
10. **Agentes podem gerenciar subordinados.**
11. **Agentes não podem ultrapassar sua autoridade.**
12. **A hierarquia deve ser visual.**
13. **A hierarquia pode evoluir dinamicamente.**
14. **Agentes temporários não devem poluir a organização permanente.**
15. **Modelos e agentes são conceitos separados.**
16. **Ferramentas e agentes são conceitos separados.**
17. **O usuário deve poder acompanhar o trabalho.**
18. **O sistema deve funcionar tanto para uso pessoal quanto empresarial.**
19. **A experiência inicial deve ser simples.**
20. **A complexidade deve aparecer apenas quando necessária.**
21. **Open source deve continuar sendo parte importante da identidade do produto.**
22. **Self-hosting e hospedagem oficial devem coexistir.**

---

# 49. Ideia central resumida

O projeto não deve ser apenas:

> "Um chatbot com vários agentes."

Ele deve ser:

> **Um sistema operacional de agentes de IA.**

O usuário começa conversando com uma IA normalmente.

Quando precisar de especialização, cria um agente.

Quando precisar de colaboração, cria vários.

Quando precisar de organização, cria uma hierarquia.

Quando precisar de automação, deixa os agentes trabalharem autonomamente.

Quando precisar programar, utiliza o Code.

E todos esses ambientes utilizam **os mesmos agentes globais, as mesmas ferramentas e o mesmo ecossistema de modelos**.

A visão final é uma organização digital na qual:

```text
                 USUÁRIO
                    │
              PRINCIPAL AGENT
                    │
       ┌────────────┼────────────┐
       │            │            │
   BUSINESS       CODE        PERSONAL
   MANAGER       MANAGER       MANAGER
       │            │
       │      ┌─────┼─────┐
       │      │     │     │
       │   BACKEND FRONT SECURITY
       │
   SPECIALISTS
```

E essa organização pode **conversar, criar, delegar, executar, revisar, monitorar, aprender e se reorganizar**, sempre dentro das permissões definidas.

---

# 50. Próxima etapa conceitual

Esta especificação deve servir como base para as próximas decisões do projeto:

- modelo de dados dos agentes;
- modelo de permissões;
- sistema de tarefas;
- protocolo de comunicação entre agentes;
- interface visual da hierarquia;
- interface dos três modos;
- sistema de execução/autonomia;
- integração com o núcleo de Code/OpenCode;
- sistema de modelos;
- sistema de ferramentas;
- automações e mensagens proativas;
- hospedagem/self-hosting;
- planos e monetização.

Essas decisões devem ser feitas posteriormente sem perder os princípios definidos neste documento.

---

# Parte 2 — O Warden hoje × a visão

> Escrito em 2026-10-04 a partir do que `ROADMAP.md`, `PENDING.md` e `ARCHITECTURE.md` registram. **Existe** = feito e
> usado; **Parcial** = há o mecanismo mas não o que a visão descreve; **Não existe** = nada ainda. "Falta" diz o que a
> visão pede além do que há.

| Seção da visão | O que o Warden tem hoje | Falta | Onde ver |
|---|---|---|---|
| §2, §16, §46 — agentes globais | **Parcial.** `[[agents]]` no `config.toml` (id, persona, modelo padrão, tools permitidas), globais ao workspace. Usados no desktop, no CLI, no hub (web, celular, extensão), nas tarefas agendadas e nos webhooks. Um membro tem agentes próprios e compartilhados. | O mesmo agente no modo de código: o projeto de código roda o opencode, não um agente do Warden; um agente do Warden o comanda pela tool `code_task` (Sessão 180, só no hub, não no desktop). | P45, P46, P84, P87, P103 |
| §3 — três modos (Chat, Agents, Code) | **Parcial.** Chat existe (conversas, projetos, pasta de trabalho) em todos os clientes. Code existe como projeto de código sobre o opencode. O modo **Agents** existe (linha de §6–7 abaixo). | A troca rápida entre os três como módulos. | P103, P89, P121 |
| §4–5 — chat sem agente, com agente, troca | **Feito, menos o contexto da troca.** O hub, a web e o desktop aceitam conversa sem agente (P124); o desktop troca de agente pelo cabeçalho, com as próximas mensagens falando como o novo. O modelo se troca por conversa no desktop (modelo ou combo); no hub o modelo vem do agente. | Não conferi na Sessão 180 o que acontece com o contexto do modelo ao trocar de agente no meio. | P45, P90, P124 |
| §5, §17, §47 — o Chat entrega o trabalho a um agente de programação e acompanha | **Parcial.** `delegate_to_agent` e `delegate_task`, síncronos ou em segundo plano (`jobs`). O destino é um agente do Warden, ou o motor de código pela tool `code_task` (Sessão 180, só no hub), que devolve a resposta e o `session_id` ao agente. | Acompanhar o trabalho do motor ao vivo no Chat (a `code_task` só devolve o resultado no fim) e o desktop. | P46, P62, P103 |
| §6–7 — modo Agents, canal persistente por agente | **Existe** (P121, sessões 165 a 175). Cada agente tem **um canal fixo**, a conversa principal dele com a pessoa, numa tela de contatos na web, no desktop (com hub), na extensão e no celular; os recados "A → B" e as execuções `task-*` do agente ficam numa lista dentro do contato dele (decisão da sessão 175). | Vista numa janela real. | P121 |
| §8 — agentes que iniciam mensagens | **Existe, em boa parte** (P121, Sessões 165 a 169). Tarefas agendadas (cron, a cada, uma vez) e webhooks rodam um agente sem prompt; a tool `message_user` e a autorização `[[outreach]]` deixam um agente autorizado escrever no canal dele (12 por hora), com envio externo para Telegram e WhatsApp pela caixa de saída e aviso de mensagem nova nos quatro clientes. | Monitoramento contínuo (hoje só por tarefa agendada ou webhook); vista numa janela real. | P92, P105, P121 |
| §9–10, §34 — agentes conversam, de forma observável | **Parcial.** `message_agent` ("funcionários"): um recado numa conversa "A → B" que o usuário vê e na qual entra; o colega responde em segundo plano. Desde a Sessão 180 ele pode **perguntar de volta** a quem o chamou, no meio da resposta (tool `ask_back`, até 3 por recado; a pergunta e a resposta ficam na conversa "A → B"). Ele ainda não manda um recado novo por conta própria. | Conversa de equipe entre vários agentes (lateral e hierárquica por permissão), "quem está trabalhando agora". | P87 |
| §11–13 — Code com um Programming Manager | **Parcial.** Projeto de código com o opencode: modos manual, aceitar edições, aceitar tudo e plano; cancelar a tarefa; eventos ao vivo. Um agente comum com persona de gerente comanda o opencode pela `code_task` (decidido e feito na Sessão 180, só no hub). | Várias sessões do opencode ao mesmo tempo, cada uma com o seu cargo; o desktop; o gerente escolher o modelo do opencode (hoje é o do hub). | P103, P89 |
| §14–15 — agentes temporários (workers) | **Parcial, mais perto.** `delegate_task` cria um sub-agente anônimo que some ao fim do turno (não polui a lista), com teto de chamadas por turno e fila em segundo plano. Desde a P123 (Sessão 149) a delegação em segundo plano aceita um `name` para o worker temporário, e cada uma aparece em "Agent work" (web e desktop) com responsável, estado, modelo e tokens. | Um papel (cargo) para o worker, e CLI, celular e extensão sem a tela. | P46, P60, P123 |
| §18–19, §31 — hierarquia, visual e visão de organização | **Existe** (P120, sessões 147 a 159; tabela atualizada na sessão 178). `role` e `reports_to` no `[[agents]]` (só do dono, validados contra ciclos e superior inexistente), árvore em todos os clientes que se edita (cargo, superior, adicionar, remover, arrastar), "Chat" e "Tasks" por nó e a atividade de cada agente. Um membro com o acesso que o dono der (nenhum, ver ou editar) vê ou edita **a mesma árvore**, sem a chave do hub. | A árvore de um membro com cargo próprio nos agentes dele (decidido que não: é uma só); membro com acesso na extensão, no celular e no desktop. | P120 |
| §20–21, §24–25 — agentes administram agentes, escopo de autoridade | **Parcial, e a tensão foi decidida** (P120, sessão 148). `manage_agents` só lista, edita e apaga os **subordinados** do chamador, nunca ele mesmo, o superior ou um par; um subordinado com poder só é dele se o chamador também tem o poder (**o teto é o gerente**); o agente criado reporta ao criador e nasce sem poderes e só com tools de leitura; ligar poderes é só de um humano e toda mudança espera o "sim". `delegate_to_agent` alcança só os subordinados de quem está na hierarquia. | Substituir e dispensar com mais ergonomia, workers temporários com nome (P123). | P46, P120, Sessões 80 a 83, 148 |
| §22 — permissões explícitas | **Parcial.** Flags `can_delegate_to_agents`, `can_manage_agents`, `can_message_agents`, `can_manage_tasks`; `allowed_tools`; shell e SSH com aprovação; limites de gasto por agente; quais agentes cada nó e cada pessoa usam. | Já existem como permissão própria: "enviar mensagens ao usuário" (`[[outreach]]`), "escolher modelos" (`delegation_models`, P123), "iniciar tarefas" (`can_start_tasks`) e "criar agentes temporários" (`can_create_workers`, ambas da Sessão 180). | "Pode executar código" e "acessar serviços externos" como flags (hoje são as tools e as categorias de risco). | P46, P84, P93, P121, P122, P123 |
| §23 — cargos | **Existe** (P120). O cargo é texto livre (`role`), mas a **posição** (`reports_to`) tem efeito: define quem um agente gerencia e a quem delega (escopo de autoridade, linha de §20–21). | Responsabilidades e permissões atreladas ao cargo em si (hoje as permissões são do agente, não do cargo). | P120 |
| §26–27 — modelo por tarefa | **Parcial, quase.** Modelo padrão por agente (`provider_id`), combos com reserva e troca de modelo por conversa no desktop. Desde a P123 o gerente **escolhe o modelo de cada delegação** (`model`, id de provedor ou combo, só entre os que a pessoa liberou). | Políticas nomeadas (rápido, barato, raciocínio, código, multimodal) no config; um modelo real escolhendo (só modelos roteirizados foram testados). | P90, P123 |
| §28, §45 — ferramentas separadas dos agentes | **Existe.** Tools por agente, MCP, SSH, nós (shell, arquivos, modelos), skills. | Uma visão única de "Tool Hub". | P47, P93, P16 |
| §29–30 — delegação com tarefas e estados | **Existe, em boa parte** (P123, Sessões 149 a 151). Toda delegação em segundo plano é um registro (`agent_tasks.jsonl`) com responsável, objetivo, estado (pendente, em andamento, aguardando agente, concluído, falhou, cancelado), modelo, resultado, tokens, subtarefas e progresso; agentes nomeados abrem subtarefas. Tarefas agendadas seguem com o estado delas (P92). | Prioridade, delegação síncrona sem registro, CLI/celular/extensão sem tela. ("Pausado" já existe, Sessão 154.) | P46, P92, P123 |
| §32 — feed de atividade | **Existe** (P121, sessões 171 a 174). Derivado dos arquivos que já existem (tarefas delegadas, recados entre agentes, mensagens que o agente escreveu primeiro, execuções de tarefa agendada e de webhook, agentes criados ou removidos por outro agente), sem log novo de eventos; aba "Atividade" na web, no desktop, na extensão e no celular. | Vista numa janela real. | P121 |
| §36 — o usuário como operador da organização | **Parcial.** Multiusuário: dono e membros do workspace. | O usuário como o topo do organograma. | P84 |
| §37 — autonomia configurável (1 a 5) | **Existe** (P122). Um nível **por agente** de 1 a 5 (1 só responde, 2 sugere, 3 pede antes de cada ação que muda algo, 4 age sozinho, 5 também gerencia os subordinados sem perguntar, Sessão 180), aplicado em código; um alvo de delegação nunca recebe mais do que quem o chamou. Mais a aprovação por ação (SSH, `manage_agents`, pausa por limite de gasto), a **aprovação por categoria de risco** (Sessão 146, `approval_required`, classificação por `[[tool_categories]]`, tabela e dicas do MCP) e os modos do projeto de código. | Editor de `[[tool_categories]]` nas telas; "sempre permitir" por categoria. | P4, P103, P122 |
| §38 — aprovação humana | **Existe.** Modal em todos os clientes, com "sempre permitir nesta conversa". | Categorias como gastar dinheiro, mensagem externa e publicar código. | P47, P46 |
| §39 — a hierarquia não é contornável por outro agente | **Parcial.** O alvo de `delegate_to_agent` usa as tools **dele**, não as do chefe; o agente criado nasce restrito; o colega de `message_agent` não manda de volta. | O modelo geral (posição + permissões + escopo + aprovação). | P46 |
| §40–42 — simples para começar, poderoso para crescer | **Parcial.** O desktop abre no chat; criar agente é por Settings ou por conversa. | A progressão guiada. | — |
| §43 — open source e hospedagem | **Parcial.** Open source e hub auto-hospedado existem; a hospedagem oficial é só registro. | Planos pagos. | P50 |
| §44–45 — ecossistema de modelos e de ferramentas | **Existe.** Gemini, OpenAI, Anthropic, compatíveis com OpenAI (Ollama...), modelo de um nó, combos; MCP. | — | P22, P90 |
| §49 — organização que "aprende" | **Parcial.** O aprendizado sugere skills a partir das conversas. | Aprender no nível da organização. | P104, P115 |

---

# Parte 3 — Decisões em aberto

Nada aqui foi decidido. São as perguntas que a visão deixa e a ordem que parece natural, para quando o usuário quiser
levar isto a `/plan`.

**Tensões com o que já foi decidido**

1. **Quem edita um agente com poder.** A Sessão 80 decidiu que um agente com poder só um humano edita, e que nenhum
   agente liga os próprios poderes. A visão (§20 a §25) deixa o gerente criar, substituir e reorganizar subordinados
   dentro do seu escopo. É preciso decidir se a regra atual vale como teto (o gerente só mexe em quem tem menos poder
   que ele) ou se muda.
2. **Agente global × agente de um membro.** A visão fala de uma organização; o Warden tem agentes do dono, agentes
   compartilhados e agentes próprios de cada membro (P84). A hierarquia é do workspace ou de cada pessoa?
   **Decidido (Sessão 180)**: uma árvore só, a do dono, no workspace; o membro com acesso (nenhum/ver/editar) mexe nela,
   e os agentes próprios dos membros continuam fora dela (sem cargo, sem superior). Mini-árvore por membro e membro
   dentro da árvore única foram descartados por ora (o segundo cruza com a privacidade e com o teto de autoridade).
3. **O modo de código não usa agentes do Warden.** O projeto de código roda o opencode (P103). A visão quer um
   Programming Manager do Warden coordenando o trabalho de código. O gerente comanda o opencode como ferramenta, ou
   há subagentes de programação do próprio Warden (P89)?
   **Decidido (Sessão 180)**: o opencode como ferramenta. Um agente do Warden (o gerente é um agente comum, com persona
   de gerente) manda tarefas a um projeto de código por uma tool (`code_task`, feita na Sessão 180, só no hub), pela camada
   `CodeEngine`; os "subagentes de programação" são sessões do opencode, não agentes do Warden com motor próprio. Fica
   para depois: várias sessões ao mesmo tempo, cada uma com o seu cargo na árvore. Coerente com a decisão da P89 (usar
   o opencode, não construir motor).
4. ~~**Canal por agente × conversas por conversa.**~~ **Já decidida e feita (P121; decisão do usuário na Sessão 165, a
   lista dos recados na 175; conferido no `PENDING.md` na Sessão 180)**: o canal **convive** com o resto. As conversas
   soltas ficam no Chat, a tela de Agents tem uma conversa fixa por agente (como um contato), e os recados "A → B" e as
   execuções `task-*` ficam numa lista dentro do contato.
5. ~~**Conversa sem agente no desktop.**~~ **Já resolvida (P124, commit `4bca2ee`; conferido no código na Sessão 180)**:
   o desktop começa com "Chat without an agent" ou com um agente, e troca de agente pelo cabeçalho a qualquer hora.

**Decisões de desenho que a visão não toma**

6. ~~**Onde mora a hierarquia**~~ **Decidida e feita (P120)**: campos `role` e `reports_to` no `[[agents]]`, validados
   por `check_hierarchy` (ciclos, superior inexistente, auto-referência); apagar um nó passa os subordinados ao superior
   dele. Vai pelo sync junto do `config.toml`. Os agentes dos membros ficam fora (decisão da Sessão 180).
7. ~~**Autonomia por agente**~~ **Feita em boa parte (P122, P121)**: níveis 1 a 4 aplicados em `autonomy::authorize`,
   aprovação por categoria de risco, limites de gasto (P4, Sessão 177), `message_user` com limite de 12 por hora e
   envio externo, o nível 5 (Sessão 180: o gerente muda os subordinados sem o "sim") e as permissões "iniciar tarefas" e "criar agentes
   temporários" (Sessão 180). **Resta** só o que a vista cita sem flag própria (executar código, serviços externos).
8. ~~**Tarefa como objeto**~~ **Decidida e feita (P123)**: um modelo próprio (`agent_tasks.jsonl`), separado das
   tarefas agendadas (P92), com "pausado" (Sessão 154) e políticas nomeadas de modelo (Sessão 153). **Resta**:
   prioridade e a tela em CLI, celular e extensão. (A auditoria da Sessão 180 tinha dado "pausado" como faltando; estava
   no `ARCHITECTURE.md`.)
9. **Hospedagem e planos** (§43): ainda só registro, ver P50.

**Ordem sugerida** (estado conferido na Sessão 180: 1 a 4 e 7 feitos; o 5 feito na Sessão 180 (nível 5 e as permissões "iniciar tarefas" e "criar agentes temporários"); do 6 a `code_task` está feita e resta a parte de várias sessões)

1. Campos de cargo e superior nos agentes, e uma **visão de organização só de leitura** (a árvore, abrir a conversa do
   nó). Dá a base visual sem mudar nenhuma regra.
2. **Regras de autoridade** aplicadas em `manage_agents` e `delegate_to_agent` pelo escopo (depende da decisão 1).
3. **Modo Agents**: o canal persistente por agente, com os recados entre agentes à vista.
4. **Feed de atividade** e estados de tarefa.
5. **Autonomia por agente** e as permissões novas (escolher modelo, iniciar tarefa, mensagem ao usuário).
6. **Gerente de código** e modelo por tarefa.
7. **Editor visual da hierarquia** (criar, mover, trocar o superior).
