# F4: plano de implementação (build, bandeja e instalação)

Escrito em 2026-11-18, a partir da `main` em `9cb9802` (Gate 3 + correções da
decisão 0007, versão 0.3.0). Branch: `win32/f4-build`. Critérios de saída:
`docs/gates.md`, seção F4. Desenho de referência: `docs/plan.md`, seções
"Tray e timer", "Build e instalação" e "Orçamento de recursos".

Objetivo da fase: um `Focus.exe` que sobe com o Windows direto na bandeja,
grava o dia inteiro sem ninguém olhar, abre a janela sob demanda e cabe no
orçamento de recursos, medido linha por linha no build de release.

## 1. Ponto de partida: o que o código faz hoje

O exe atual tem **dois modos que nunca rodam juntos**:

| Modo | Como sobe | O que faz | O que falta para a F4 |
|---|---|---|---|
| Monitor | `focus.exe [--db p]` | `main.rs`: hooks `SetWinEventHook`, janela oculta para energia e sessão, timers de flush e ociosidade, leitor do Claude Code numa thread | Sem bandeja, sem janela, subsistema `console` (abre um console ao iniciar), sai com Ctrl+C |
| UI | `--ui`, `--demo`, `--ui-show`, `--ui-cycle`, `--diag-drag` | `ui/selftest.rs`: abre a janela, roda um laço até ela fechar e encerra o processo | Fechar a janela encerra o app; não há monitor rodando junto |

Lacunas encontradas no código que afetam a F4 (não estão nos critérios do
gate, mas o teste de 8 h de "uso real" depende delas):

1. **O monitor ignora as Configurações.** `recorder.rs` e `main.rs` não leem
   `monitor_enabled`, `browser_tabs`, `keep_active_when_busy`,
   `excluded_apps` nem `excluded_title_patterns`; `idleMinutes` é lido só
   na partida. O leitor do Claude respeita `claudeSessions`.
2. **URL do navegador não é gravada.** `recorder.rs` grava `url: ""`
   sempre. A decisão 0003 aprovou a leitura via UI Automation, mas ela só
   existe no `examples/uia_probe.rs`.
3. **Escrita no banco na thread do monitor** (item de backlog "thread
   dedicada de banco e EcoQoS", que o `gates.md` manda fazer no início da F4).
4. Watchdog de 60 s, mutex de instância única, `RegisterApplicationRestart`,
   backup diário e `quick_check` semanal (pedidos pelo plano e verificados no
   gate da F5) não existem.

Pontos a favor: `popup::open` já aceita `owner: None` (o flyout pode usar o
mesmo host de popup sem a janela principal aberta), o manifesto já é
embutido pelo `build.rs`, as DLLs gráficas já são delay-load, e o binário
está em 2,15 MB.

## 2. Decisões de arquitetura da fase

Cada uma vira uma nota curta em `docs/decisions/` quando for implementada
(numeração a partir de 0008; a nota do gate fica com o próximo número livre).

**D1. Três threads de fundo e uma de UI, cada uma com sua própria janela
oculta.**

| Thread | Dona de | Acorda com |
|---|---|---|
| UI (principal) | janela oculta da bandeja (classe `FocusTrayWindow`), ícone, menu, flyout, mini timer, janela principal | clique na bandeja, `TaskbarCreated`, mensagens das outras threads, 1×/min com timer rodando (anel e tooltip) |
| Monitor | hooks WinEvent, janela oculta de energia e sessão (a `FocusHiddenWindow` atual), timers de debounce, flush e ociosidade | eventos de foco e título, energia, sessão, timers coalescíveis |
| Banco | a única conexão de escrita | comandos por canal |
| Claude Code | leitor de logs (já existe) | `ReadDirectoryChangesW` |
| URL (UIA) | thread COM MTA da leitura da barra de endereço | pedido do monitor quando o título de um navegador muda |

Por que o monitor sai da thread principal: laços modais (mover e
redimensionar a janela, `TrackPopupMenu`, `IFileSaveDialog`) descartam
mensagens de thread com `hwnd` nulo, e os timers do monitor são exatamente
isso. Com o monitor numa thread própria, o código atual de `main.rs` muda
pouco: vira `monitor::spawn()` e troca o `PostThreadMessageW(WM_QUIT)` do
Ctrl+C por um pedido de parada vindo da UI.

A janela da bandeja precisa ser **top-level oculta, não `HWND_MESSAGE`**:
janelas só de mensagem não recebem broadcasts, e `TaskbarCreated` é um.

**D2. Instância única e canal de comando.** Mutex `Local\Focus-<SID>`. A
segunda instância procura a `FocusTrayWindow` e manda uma mensagem
registrada (`RegisterWindowMessageW`): `Focus.Show` (padrão),
`Focus.Quit` (`--quit`) ou nada (`--background`, sai em silêncio). Antes de
mandar `Show`, chama `AllowSetForegroundWindow(pid)` para a primeira
instância poder trazer a janela para frente. Os nomes da classe e das
mensagens viram contrato estável, porque o `install.ps1` depende deles entre
versões. Modos de diagnóstico (`--demo`, `--ui-*`, `--diag-drag`) não pegam
o mutex.

**D3. Subsistema `windows`.** Sem isso a chave Run abre um console a cada
login. Os modos de diagnóstico chamam `AttachConsole(ATTACH_PARENT_PROCESS)`
para continuar imprimindo no terminal; os scripts que os chamam passam a
esperar o processo (`| Out-String` ou `Start-Process -Wait`), porque o
PowerShell não espera exe GUI. Todo `eprintln!` de caminho de fundo vai para
o log em arquivo.

**D4. Ícone da bandeja sem Direct2D.** Com a janela fechada a pilha gráfica
não pode carregar (orçamento < 5 MB). O ícone é rasterizado em Rust puro
(anel e disco com antialiasing por distância, num DIB de 32 bpp) e entregue
com `CreateIconIndirect`, no tamanho de
`GetSystemMetricsForDpi(SM_CXSMICON, dpi)`. O mesmo rasterizador gera o
`.ico` do exe (16, 20, 24, 32, 40, 48 e 256 px) por um exemplo
(`cargo run --example make_icon`), commitado em `win32/assets/focus.ico` e
embutido pelo `build.rs` como recurso (crate `embed-resource` como
build-dependency, que usa o `rc.exe` do Windows SDK já instalado).

**D5. Flyout e mini timer custam a pilha gráfica.** Os dois são desenhados
em Direct2D como o resto da UI. Pela decisão 0004, a primeira abertura
deixa ~21 MB residentes. Fica aceito: a memória fecha em < 30 MB depois da
primeira abertura de qualquer superfície (janela, flyout ou mini timer). O
tooltip e o menu de contexto são nativos e não carregam nada.

**D6. Estados do ícone.** Interpretação proposta de "parado, rodando e
pausado por ociosidade":

| Estado | Condição | Desenho |
|---|---|---|
| Parado | sem timer manual rodando | ícone base |
| Rodando | timer rodando e usuário presente | ícone base + anel que completa a cada hora, redesenhado no máximo 1×/min |
| Pausado | gravação pausada (ociosidade, bloqueio, monitor desligado nas Configurações) | ícone base esmaecido + marca de pausa |
| Erro | escrita no banco falhando (disco cheio, E/S) | marca de alerta; tooltip diz o erro |

Variante clara ou escura conforme `SystemUsesLightTheme` (a barra de
tarefas tem tema próprio), trocada em `WM_SETTINGCHANGE`.

## 3. Etapas

Ordem pensada para ter um exe utilizável cedo: E1 já dá um app de bandeja
que grava; as etapas seguintes acrescentam peças sem quebrar o anterior.
Cada etapa termina com `scripts/check.ps1` passando e commits pequenos (os
assuntos sugeridos estão em cada etapa). Tamanho relativo: P, M, G.

### E0. Preparação (P)

- Mover o item "F4" do quadro para `In Progress`; mover também o item de
  backlog "Thread dedicada de banco e EcoQoS" (feito na E3).
- Este documento commitado na branch.
- Commit: `docs(win32): add F4 implementation plan`.

### E1. Processo único: monitor + bandeja + janela sob demanda (G)

O núcleo da fase. Depois dela, `focus.exe` sem argumentos é o app.

1. Extrair o corpo de `main()` para `src/monitor.rs` com
   `spawn(db_path) -> MonitorHandle` (thread própria, janela oculta própria,
   mesmo comportamento). `MonitorHandle::stop()` posta o pedido de parada e
   faz `join` (fechar execução, `optimize`, `checkpoint`).
2. `src/app.rs` (raiz do processo): mutex (D2), criação da
   `FocusTrayWindow`, laço de mensagens com `GetMessageW` que **não termina
   quando a janela principal fecha**, só com `Quit`.
3. Mensagens registradas `Focus.Show` / `Focus.Quit` e as flags
   `--background`, `--quit`, `--version`.
4. Janela principal aberta sob demanda: reaproveitar `BgStore` /
   `run_bg_screen` de `selftest.rs`, sem o laço próprio. Fechar a janela
   (botão X, Alt+F4) chama `window::close()`, que já destrói tudo, e o
   processo continua.
5. Subsistema `windows` + `AttachConsole` nos modos de diagnóstico (D3);
   ajustar `check.ps1` e os comandos de medição que chamam o exe.
6. Fim de sessão: cada thread trata o próprio `WM_ENDSESSION` na própria
   janela oculta; a UI espera o monitor gravar (com limite de tempo) antes
   de retornar.

Testes: mutex e roteamento de flags como funções puras (`parse_args`,
`decide_action(already_running, flags)`); teste de integração que sobe o exe
com `--db` temporário e `--background`, manda `--quit` e confere saída
limpa e execução fechada no banco.

Commits sugeridos: `Move foreground monitor onto its own thread`,
`Add single-instance mutex and show/quit window messages`,
`Keep the process alive after the main window closes`,
`Switch to the windows subsystem and attach a console for diagnostics`.

### E2. Monitor obedece às Configurações; URL e watchdog (G)

Fecha as lacunas 1 e 2 da seção 1.

1. Recarga de configurações: depois de salvar em Configurações, a UI posta
   `RELOAD_SETTINGS` ao monitor, que relê `idle_minutes`,
   `monitor_enabled`, `keep_active_when_busy`, `browser_tabs` e as
   exclusões.
2. `monitor_enabled = false` vira um motivo de pausa novo em `presence.rs`
   (`USER_PAUSED`), com a mesma semântica dos outros: fecha a execução,
   retoma com `resync`.
3. Exclusões: app excluído (por nome amigável ou nome do exe, sem
   diferenciar maiúsculas) ou título com palavra excluída fecha a execução
   atual e não grava nada até o foco sair dele. Nenhum título excluído vai
   para banco ou log. Testes de tabela no `recorder`.
4. URL via UI Automation, conforme decisão 0003 e `AGENTS.md`: thread MTA
   própria, só quando o app em primeiro plano é Chrome, Edge ou Firefox e o
   título mudou; `TreeScope_Children` encadeado, elemento da barra em cache
   por janela; timeout de 500 ms; a execução é gravada sem URL e a URL entra
   depois. Regras do plano: prefixar `https://` quando faltar, descartar
   query e fragmento, ignorar quando o campo tem foco de teclado, nada em
   janelas InPrivate/anônimas, nada em navegador elevado. Respeita
   `browser_tabs`.
5. Watchdog de 60 s pegando carona no timer de flush: compara
   `GetForegroundWindow()` com o último conhecido e ressincroniza.

Commits sugeridos: `Reload monitor settings when Settings are saved`,
`Pause recording when the monitor is turned off in Settings`,
`Skip excluded apps and window keywords in the recorder`,
`Read browser address bar via UI Automation on title change`,
`Resync the foreground window every flush as a hook watchdog`.

### E3. Thread de banco e EcoQoS (M)

Item de backlog, critérios dele no `gates.md`.

1. `src/db_thread.rs`: thread dona da conexão de escrita, comandos por
   `mpsc` (`RecordFocus`, `Close`, `Tick`, `Checkpoint`, `StartTimer`,
   `StopTimer`, ...), respostas por canal quando a UI precisa do resultado.
   A UI continua lendo pela própria conexão (WAL permite).
2. `SetThreadInformation(ThreadPowerThrottling)` e
   `MEMORY_PRIORITY_LOW` nas threads de monitor, banco, Claude e URL; a
   thread de UI desliga o estrangulamento só enquanto a janela estiver
   aberta.
3. Teste com escritor lento (comando que dorme 2 s): o monitor continua
   aceitando eventos e o debounce não atrasa.
4. Medir CPU e wakeups com a janela fechada antes e depois; não podem
   piorar (CSV).

Commits sugeridos: `Move database writes onto a dedicated thread`,
`Enable EcoQoS and low memory priority on background threads`.

### E4. Ícone da bandeja (M)

1. `src/tray/icon.rs`: rasterizador (D4), funções puras testadas por pixel
   (anel em 0, 25, 50 e 100%, variante clara e escura, tamanhos 16 a 48).
2. `examples/make_icon.rs` + `assets/focus.ico` + recurso no `build.rs`.
3. `src/tray/mod.rs`: `Shell_NotifyIconW` com `NOTIFYICON_VERSION_4`,
   identidade por `uID` (não GUID, que fica preso ao caminho do exe),
   recriação em `TaskbarCreated`, tooltip truncado em 128 caracteres
   (`NIF_SHOWTIP`), estados da D6.
4. Timer coalescível de 60 s ligado só com timer rodando (anel e tooltip);
   nenhum timer com timer parado.
5. Recalcular tamanho e variante em `WM_SETTINGCHANGE`,
   `WM_DISPLAYCHANGE` e `WM_DPICHANGED`.

Teste manual registrado na nota: matar o `explorer.exe` e ver o ícone
voltar.

Commits sugeridos: `Rasterize tray icon states without Direct2D`,
`Embed multi-size application icon`, `Add notification area icon with
state, tooltip and TaskbarCreated recovery`.

### E5. Menu, flyout, mini timer e notificações (G)

1. Menu de contexto nativo (`TrackPopupMenuEx`, com
   `SetForegroundWindow` antes e `PostMessage(WM_NULL)` depois): iniciar ou
   parar o timer, iniciar num dos 5 projetos recentes (submenu), pausar ou
   retomar a gravação, abrir o Focus, mostrar o mini timer, sair. Textos em
   `strings.rs`.
2. Flyout (clique esquerdo, `NIN_SELECT` / `NIN_KEYSELECT`): popup sem dono
   via `popup::open(None, ...)`, ancorado em `Shell_NotifyIconGetRect` e na
   área de trabalho do monitor; descrição, projeto, tempo decorrido,
   iniciar e parar; fecha ao perder a ativação. Acessível (árvore com nomes,
   teclado) como as outras telas.
3. Mini timer: janela pequena sempre no topo, arrastável, posição salva,
   desligada por padrão, ligada pelo menu e pelas Configurações.
4. Sincronização: timer iniciado ou parado pela bandeja atualiza a janela
   principal aberta (sinal "dados mudaram" para o `AppShell`), e vice-versa
   (a UI avisa a bandeja para trocar o ícone).
5. Notificações por balão (`NIF_INFO`):
   - primeiro fechamento da janela ("O Focus continua na bandeja"),
     uma vez só, com flag em settings;
   - erro de escrita no banco (E7);
   - timer rodando durante um sono maior que o limite de ociosidade: o
     balão não tem botões, então o clique nele (`NIN_BALLOONUSERCLICK`)
     abre o flyout com "Manter" e "Descartar". Ignorado, o tempo fica (nunca
     perder dado em silêncio).
6. Diagnóstico novo `--tray-cycle N`: abre e fecha o flyout N vezes e
   imprime a memória, como o `--ui-cycle`.

Fora da F4 (proposto para backlog da F5): atalho global com
`RegisterHotKey`. Está no plano como opcional e não está no gate.

Commits sugeridos: `Add tray context menu with timer and recent projects`,
`Add tray flyout anchored to the notification area`,
`Add optional always-on-top mini timer`, `Notify on first close and on
sleep with a running timer`.

### E6. Início com o Windows (P)

1. `src/startup.rs`: ler e gravar `HKCU\Software\Microsoft\Windows\
   CurrentVersion\Run`, valor `Focus` = `"<exe atual>" --background`.
   Estado real lido de `...\Explorer\StartupApproved\Run` (valor binário;
   bit 0 do primeiro byte ligado = desligado pelo usuário no Gerenciador de
   Tarefas). Funções puras para interpretar o valor, com testes.
2. Toggle "Start with Windows" em Configurações, mostrando quando o
   Gerenciador de Tarefas desligou (e não religando por cima).
3. `RegisterApplicationRestart("--background", ...)` para voltar depois de
   atualização do Windows.

Commits sugeridos: `Manage the Run key and read StartupApproved state`,
`Add Start with Windows toggle to Settings`,
`Register for restart after Windows updates`.

### E7. Robustez para rodar sem supervisão (M)

1. `src/log.rs`: log em `%LOCALAPPDATA%\Focus\logs\focus.log`, rotação em
   1 MB (mantém `focus.1.log`), carimbo de hora, **nunca** título, URL ou
   nome de app (a API só aceita categoria e código de erro).
2. Panic hook (roda antes do `abort` do perfil de release) +
   `SetUnhandledExceptionFilter`, ambos chamando `MiniDumpWriteDump`
   (`MiniDumpNormal`) em `%LOCALAPPDATA%\Focus\crashes`, guardando os 5 mais
   recentes; `dbghelp.dll` delay-load. Flag oculta `--crash-test` para
   provar que o dump sai.
3. Falha de escrita (`SQLITE_FULL`, E/S): pausar a gravação, ícone em erro,
   balão, nova tentativa a cada 5 min; `SQLITE_BUSY` só tenta de novo.
4. Backup diário com `VACUUM INTO` em `%LOCALAPPDATA%\Focus\backups`,
   guardando 7, na thread de banco em `THREAD_MODE_BACKGROUND_BEGIN`;
   `PRAGMA quick_check` semanal, resultado no log. Ver risco R3.

Commits sugeridos: `Add rotating file log without private data`,
`Write minidumps on panic and unhandled exceptions`,
`Pause recording and retry on database write failures`,
`Add daily backup and weekly quick_check`.

### E8. `scripts/install.ps1` (M)

Parâmetros: `-Exe` (padrão `win32\target\release\focus.exe`), `-NoLaunch`,
`-Uninstall`.

1. Valida o exe de origem (existe, < 5 MB, `--version` responde).
2. Se o Focus está rodando: `Focus.exe --quit` e espera até 10 s pela
   saída do processo. Se não sair, **para e avisa** (não mata: matar perde
   até 60 s de gravação).
3. Copia para `%LOCALAPPDATA%\Programs\Focus\Focus.exe.new` e troca com
   `Move-Item -Force`; mantém a versão anterior como `Focus.prev.exe` para
   voltar atrás.
4. Recria o valor `Focus` em `HKCU\...\Run` com `--background` (o estado
   em `StartupApproved` é do usuário e não é tocado).
5. Abre o app com `--background`, a menos que `-NoLaunch`.
6. `-Uninstall`: `--quit`, remove o valor Run e a pasta do programa;
   **nunca** toca em `%LOCALAPPDATA%\Focus` (dados), só imprime onde eles
   ficaram.

`scripts/test-install.ps1`: roda o install duas vezes e compara um retrato
do estado (hash do exe instalado, valor da chave Run, número de processos
`Focus` = 1, listagem de nomes e tamanhos da pasta de dados antes e depois
igual). Saída só com booleanos e números, commitada em
`docs/measurements/`.

Commits sugeridos: `Add install script with graceful quit and atomic
replace`, `Add install idempotency test`.

### E9. Instrumentação das medições (P)

1. `measure.ps1`: coluna nova de trocas de contexto por segundo somando
   `\Thread(focus/*)\Context Switches/sec` (`Get-Counter`, sem instalar
   nada); CPU média sai do delta de `cpu_seconds` sobre o tempo.
2. Log de partida: `startup tray_ms=` medido do horário de criação do
   processo (`GetProcessTimes`) até o `Shell_NotifyIconW` retornar; ordem de
   partida com mutex → bandeja → banco e monitor para caber em 150 ms.
3. Log de abertura: `open first_frame_ms=` do clique na bandeja até o
   primeiro quadro (caminho novo, diferente do `--ui-show`).
4. `scripts/measure-startup.ps1`: 10 partidas frias, CSV com os tempos.

Commit sugerido: `Measure context switches, start-to-tray and tray-open
times`.

### E10. Medição final, 8 h e gate (M, quase todo tempo de relógio)

Tudo no exe de release instalado pelo `install.ps1`:

| Linha do orçamento | Meta | Evidência |
|---|---|---|
| Privada, janela nunca aberta | < 5 MB (teto acordado 8 MB), curva plana | `measure.ps1 -Hours 8`, janela sem abrir |
| Privada depois da primeira abertura | < 30 MB, plana | `--ui-cycle 50 --all-screens` + `--tray-cycle 50` no exe final |
| Working set, janela fechada | < 10 MB | mesmo CSV de 8 h |
| CPU média, janela fechada | < 0,05% | delta de CPU no CSV de 8 h |
| Wakeups | < 1/s em média | coluna de trocas de contexto |
| Escrita em disco | < 1 MB/h | `io_write_bytes` no CSV |
| Janela aberta parada | < 60 MB WS, 0% CPU | repetir a medição de tela aberta da 0007 |
| Arrastar no calendário | 60 fps a 4K/200% | `--diag-drag` no exe final |
| Partida até a bandeja | < 150 ms | `measure-startup.ps1` |
| Abrir a janela fria | < 200 ms | log `open first_frame_ms` |
| Binário | < 5 MB | `check.ps1 -Release` |

Depois: dia de trabalho real de 8 h com o exe iniciado pelo Windows,
`measure.ps1` rodando e nota de qualquer detecção do antivírus (no repo,
sem detalhes da configuração de segurança da máquina).

Ritual do gate (`gates.md`): nota `docs/decisions/NNNN-gate-4.md` com os
números, exportar o plano se mudou, versão 0.4.0 com `Cargo.lock`, merge
`--no-ff` na `main`, tag `win32-gate-4`, push, branch `win32/f5-uso`,
item do quadro em `Done`.

## 4. Dependências entre etapas

```
E0 → E1 → E2 ─┐
        │     ├→ E9 → E10
        ├→ E3 ┤
        ├→ E4 → E5
        ├→ E6
        ├→ E7 (o backup e o tratamento de erro usam a thread de banco da E3)
        └→ E8 (precisa de --quit e --version da E1)
```

E1 é pré-requisito de tudo. Depois dela, E2, E3, E4, E6 e E8 mexem em
arquivos diferentes e podem andar em paralelo; E5 depende de E4, E7 de E3.

## 5. Riscos

| # | Risco | Mitigação |
|---|---|---|
| R1 | Subsistema `windows` quebra os scripts que leem a saída do exe | `AttachConsole` + scripts esperando o processo; ajustado e testado na E1 |
| R2 | Flyout deixa ~21 MB residentes após o primeiro clique na bandeja | Aceito pela D5 e coberto pelo teto de 30 MB; se incomodar no uso real, o flyout pode virar menu nativo na F5 |
| R3 | Backup diário com `VACUUM INTO` escreve o banco inteiro e pode estourar < 1 MB/h num banco grande | Medir com o banco real na E7; se estourar, backup só quando o banco mudou e decisão registrando que o backup fica fora da média horária |
| R4 | Antivírus estranha o exe novo: início automático, leitura de títulos e UIA no mesmo processo | Teste de 8 h com o exe instalado; bloqueio nunca é contornado em código, vai para o usuário pedir exceção (regra do gate) |
| R5 | Leitura de URL aumenta CPU e wakeups de fundo | Medir antes e depois na E2; o toggle `browser_tabs` desliga |
| R6 | Fim de sessão com várias threads: o Windows mata o processo antes do monitor gravar | Monitor trata o próprio `WM_ENDSESSION` na própria janela; UI espera com limite; teste com `shutdown /l` registrado na nota |

## 6. Decisões que dependem do usuário

1. As lacunas 1 e 2 (monitor seguir as Configurações, URL via UIA) entram
   na F4, como a E2 propõe, ou ficam para a F5? Recomendação: F4, porque o
   orçamento de 8 h precisa medir todo o trabalho de fundo do exe final.
2. Atalho global (`RegisterHotKey`) fica para o backlog da F5 (proposto
   acima).
