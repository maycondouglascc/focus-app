<!-- Exported from the planning doc at Gate 1. The line describing this machine's security setup was removed because the repository is public. -->

# Focus para Windows: plano de reescrita em Win32 nativo

·

Reescrever o Focus, só para Windows 11 e só para este computador, como um único executável Rust sobre Win32, com monitor orientado a eventos e UI desenhada em Direct2D, para ficar abaixo de 5 MB de RAM e ~0% de CPU com a janela fechada.

## Decisões de base

Rust sobre a API Win32 pura, um único processo e nenhuma dependência de runtime. Cada escolha abaixo foi feita pelo custo em segundo plano, porque o app passa ~99% do dia com a janela fechada.

| Camada | Escolha | Por quê | Alternativa descartada |
|---|---|---|---|
| Linguagem | Rust (estável), crate windows da Microsoft | Segurança de memória num processo que roda 24/7; bindings oficiais e completos de Win32, COM e WinRT | C++ com WIL: mesma performance, mas vazamentos e use-after-free viram bugs de campo |
| Processo | Um executável, sem processos filhos | Nada de IPC, um só working set | Serviço + app: serviços rodam na sessão 0 e não enxergam a janela do usuário |
| CRT | +crt-static | Sem VC++ Redistributable para instalar | DLL do CRT: mais um pré-requisito |
| Monitor | SetWinEventHook fora de contexto | O Windows avisa a troca; zero polling, sem DLL injetada | SetWindowsHookEx: injeta DLL e parece keylogger para antivírus |
| URL do navegador | UI Automation, só quando o título muda | Única via sem extensão no navegador | Extensão + native messaging: descartada por decisão de escopo |
| Banco | rusqlite com SQLite embutido (feature bundled) | Mesmo schema e mesmas migrações do app atual; o arquivo do Mac abre no Windows | Formato próprio: perde compatibilidade e ferramentas |
| Renderização | Direct2D + DirectWrite sobre DirectComposition | Antialiasing, texto ClearType e transparência com Mica; só existe com a janela aberta | GDI: leve, mas sem antialiasing e sem alpha correto |
| Layout | Crate taffy (flexbox em Rust puro) | Mesmo modelo mental do CSS atual, sem runtime | Layout manual por coordenadas: não escala para 6 telas |
| Campos de texto | Controles nativos RICHEDIT50W sem borda | IME, colar, desfazer, corretor e leitor de tela de graça | Editor de texto próprio sobre TSF: meses de trabalho |
| Acessibilidade | Crate accesskit (adaptador UIA) | Expõe a árvore desenhada ao Narrador e ao NVDA | Implementar providers UIA na mão |
| Ícones | Fonte Segoe Fluent Icons, que já vem no Windows 11 | Já vem no sistema, escala em qualquer DPI, zero bytes no binário | PNGs: um arquivo por escala e por tema |
| Formatação de datas e moeda | ICU do próprio Windows (icu.dll) | Formata qualquer moeda e locale sem embutir dados | Crate ICU4X: adiciona MBs ao binário |

Alvo: só Windows 11 em x64, que é o caso deste computador (build 26300, AMD64). Sem Windows 10 nem ARM64, saem os fallbacks de Mica, ícones e fontes, e sai um alvo de build.

## Arquitetura

Janela, Direct2D e acessibilidade só existem enquanto alguém olha para o app. No resto do dia o processo se resume às threads da faixa de cima e à thread do banco, todas paradas até o Windows mandar um evento.

## Inventário de funcionalidades

O app atual tem ~1.100 linhas no processo main e ~4.500 linhas de TSX mais ~4.500 de CSS na interface. A interface é de longe a maior parte da reescrita: em Win32 cada tela vira código de desenho e layout.

| Funcionalidade | Onde está hoje | Como fica no Win32 | Peso |
|---|---|---|---|
| Calendário semanal com arrastar, mover entre dias, redimensionar | WeekGrid.tsx (384), layout.ts | Canvas Direct2D com SetCapture, snap de minutos e autoscroll; o algoritmo de sobreposição é portado com os mesmos casos de teste | Alto |
| Barra do timer e editor de entrada | TimerBar.tsx, EntryEditor.tsx | Canvas + campos RichEdit nativos; seletor de hora próprio | Médio |
| Popover de atividade ("registrar como entrada") | ActivityPopover.tsx | Janela popup WS_POPUP com DirectComposition | Médio |
| Tela de atividade: heatmap, barras empilhadas, sparklines, top apps | ActivityScreen.tsx, charts.tsx (331) | Gráficos em geometria Direct2D, tooltips por hit-test | Alto |
| Relatórios por dia, semana, mês, ano e tudo; donut; árvore de entradas | ReportsScreen.tsx, EntryTree.tsx | Mesmos gráficos + lista virtualizada | Alto |
| Exportar CSV | Reports | IFileSaveDialog + escrita direta (ver edge cases de CSV na interface) | Baixo |
| Projetos: cores, cliente, valor/hora, moeda, arquivar | ProjectsScreen.tsx | Tabela desenhada + diálogo | Médio |
| Relatório por projeto | ProjectReportScreen.tsx (300) | Reaproveita os gráficos | Médio |
| Configurações | SettingsScreen.tsx | Toggles, selects; agora com início automático e exclusões | Baixo |
| Boas-vindas (primeiro uso) | Welcome.tsx | Sem passo de permissão de Acessibilidade; ganha a dica de fixar o ícone na bandeja | Baixo |
| Timer na barra de menus | tray.ts | Ícone na bandeja + flyout próprio (ver Tray e timer) | Médio |
| Monitor de app e janela | monitor.ts (JXA a cada 5 s) | Eventos Win32, sem polling | Médio |
| URL da aba | AppleScript por navegador | UI Automation | Alto (risco) |
| Ícones dos apps | JXA + NSWorkspace | SHGetFileInfo/IShellItemImageFactory no exe; logo do pacote para apps da Store | Baixo |
| Sessões do Claude Code | claudeSessions.ts (polling de 60 s) | ReadDirectoryChangesW + leitura incremental | Médio |
| Modo demo para screenshots | demoApi.ts | Flag --demo que abre um banco em memória com dados gerados | Baixo |
| Versão web (dev:web) | Vite | Deixa de existir | — |

O que muda de propósito: o passo de permissão de Acessibilidade some, porque o Windows não exige. Modo escuro e pt-BR ficam fora da primeira versão: a interface continua clara e em inglês, como hoje.

## Monitor de atividade

O monitor dorme até o Windows avisar uma troca de janela ou de título. Uma thread própria com fila de mensagens recebe EVENT_SYSTEM_FOREGROUND e EVENT_OBJECT_NAMECHANGE (filtrado para a janela em primeiro plano e OBJID_WINDOW), espera 1,5 s de estabilidade e só então fecha a execução anterior e abre a nova.

Identificar o app: GetWindowThreadProcessId → OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) → QueryFullProcessImageNameW. O nome exibido vem do FileDescription do executável, com cache por caminho e versão. A identidade estável (o antigo bundleId) é o AppUserModelID quando existir, senão o caminho do exe normalizado.

| Caso | O que acontece | Solução |
|---|---|---|
| App da Store ou UWP (Calculadora, WhatsApp, Fotos) | A janela pertence a ApplicationFrameHost.exe, não ao app | Descer até a Windows.UI.Core.CoreWindow filha com outro PID; nome e ícone pelo pacote (GetPackageFullName + manifesto) |
| App com janela travada | GetWindowTextW envia WM_GETTEXT e pode bloquear a thread | Usar InternalGetWindowText, que não envia mensagem; nunca SendMessage sem SMTO_ABORTIFHUNG |
| Janela "fantasma" de app que não responde | O Windows troca a janela por uma do dwm.exe | Detectar a classe Ghost e manter a execução do app original |
| Processo elevado (admin) em primeiro plano | OpenProcess limitado funciona; UI Automation não (UIPI) | Registrar app e título; sem URL. Não pedir elevação para o Focus |
| Processo protegido (antivírus, anti-cheat) | OpenProcess falha | Nome pela classe da janela; rótulo "App protegido" |
| Shell do Windows: área de trabalho, barra de tarefas, Alt+Tab, Iniciar, busca | Eventos de foco em janelas de passagem (Shell_TrayWnd, WorkerW, XamlExplorerHostIslandWindow, SearchHost.exe) | Lista de classes e exes de passagem: não fecham a execução atual se durarem menos de 5 s; acima disso viram "Windows" |
| Alternância rápida (Alt+Tab passando por várias janelas) | Dezenas de eventos em 1 s | Debounce de 1,5 s antes de confirmar |
| Título que muda o tempo todo | Contadores "(3) Inbox", marcador "●" do VS Code, música no Spotify, progresso no terminal | Normalizar antes de comparar (contadores, marcadores, spinners) e limitar NAMECHANGE a 1 gravação a cada 2 s por janela |
| O próprio Focus em primeiro plano | Seria contado como trabalho | Comparar PID com GetCurrentProcessId(), não o nome |
| Apps Squirrel (Slack, Discord, Teams clássico) | O caminho muda a cada versão (app-1.0.9xxx\) | Identidade pelo AUMID; senão remover a pasta de versão do caminho |
| FileDescription vazio ou genérico ("Electron", "Java(TM) Platform") | Nome inútil | Cair para ProductName, depois nome do exe; mapa de correções para os casos comuns |
| Janelas de console | O dono pode ser conhost.exe ou o shell; no Windows Terminal é WindowsTerminal.exe | Validar no spike; agrupar como "Terminal" com o título da aba como detalhe |
| Gerenciadores de senha e janelas privadas | Títulos sensíveis vão para o banco | Lista de exclusão por app (1Password, Bitwarden, KeePass por padrão) e por regex de título; sufixos InPrivate e Anônimo descartam a URL |
| Área de trabalho virtual, vários monitores | Foco continua único | Nenhuma ação: o evento de foco cobre a troca |
| Hook para de entregar eventos | Thread presa ou bug do sistema | Watchdog a cada 60 s compara GetForegroundWindow() com o último conhecido e ressincroniza; custo de uma chamada |
| Duas sessões de usuário na mesma máquina | Cada uma roda um Focus | Mutex com escopo Local\ e dados por usuário; nada é compartilhado |

Gravação: o fim da execução atual fica em memória e vai para o banco a cada 60 s e em toda troca. Hoje o app faz um UPDATE a cada 5 s, o que são ~5.800 escritas por dia sem necessidade. Em queda de energia perde-se no máximo 60 s.

## URL do navegador via UI Automation

O título da aba já vem no título da janela; só a URL exige UI Automation. A consulta roda numa thread COM MTA separada, com timeout de 500 ms, e só dispara quando o app em primeiro plano é um navegador e o título mudou. Uma consulta lenta nunca atrasa o monitor: a execução é gravada sem URL e a URL entra depois, se chegar.

| Caso | O que acontece | Solução |
|---|---|---|
| Chrome, Edge, Brave, Vivaldi, Opera | A barra de endereço é um Edit dentro da toolbar; o nome dela é traduzido ("Barra de endereço e de pesquisa" em pt-BR) | Achar pelo tipo de controle e posição, nunca pelo nome; guardar o elemento em cache por janela e refazer a busca só se falhar |
| Esquema escondido | A barra mostra github.com/foo, sem https:// | Prefixar https:// quando faltar; para chrome:// e edge:// registrar só o título |
| Usuário digitando na barra | O valor é o texto digitado, não a URL | Se o campo tem foco de teclado (HasKeyboardFocus), manter a URL anterior |
| Firefox | Barra acessível via UIA, mas a consulta liga o motor de acessibilidade do Firefox | Medir no spike; se o custo for alto, Firefox fica só com título |
| Chromium ligando o modo de acessibilidade completo | Clientes UIA podem fazer o Chrome construir a árvore de acessibilidade das páginas, gastando CPU e memória do navegador | Consultar só a árvore da UI do navegador, com TreeScope_Children encadeado, sem FindAll em Subtree e sem assinar eventos UIA; medir a memória do Chrome com e sem o Focus no spike |
| PWA e janela --app | Não há barra de endereço | Identidade pelo AUMID; nome do app instalado como "app" |
| Janela anônima ou InPrivate | URL sensível | Detectar pelo sufixo do título ("InPrivate", "Anônima", "Incognito") e não gravar URL nem título da aba |
| Título com perfil e contagem de abas (Edge: "Página e mais 3 páginas - Pessoal — Microsoft Edge") | Sufixos poluem o detalhe | Normalizador por navegador com testes de tabela, em pt-BR e en |
| URL com tokens na query string (links mágicos, OAuth) | Segredos vão parar no banco | Gravar só origem e caminho por padrão; query e fragmento descartados |
| Picture-in-picture e janelas de DevTools | Janelas do navegador sem barra | Tratar como o navegador, sem URL |
| Navegador elevado | UIPI bloqueia a consulta | Sem URL; sem erro na UI |

Sem extensão de navegador (decisão de escopo). Se o Gate 0 mostrar que a UI Automation pesa no navegador, a URL fica desligada e o Focus registra só o app e o título da aba.

## Ociosidade, bloqueio, suspensão e relógio

Timestamps ficam em epoch UTC em milissegundos, como hoje; durações são medidas com relógio monotônico. Uma janela de mensagens invisível (HWND_MESSAGE) recebe todos os avisos de energia e sessão.

| Caso | O que acontece | Solução |
|---|---|---|
| Ociosidade | Não há evento de "usuário ficou ocioso" | GetLastInputInfo num timer coalescível (SetCoalescableTimer) a cada 30 s com tolerância de 10 s; ao cruzar o limite, a execução termina no instante do último input, com data retroativa |
| Volta da ociosidade | Também não há evento | Só enquanto ocioso, checar GetLastInputInfo a cada 3 s (é leitura de memória compartilhada, sem syscall cara). Nunca Raw Input global: antivírus trata como keylogger |
| Contador de 32 bits | GetLastInputInfo e GetTickCount estouram após 49,7 dias ligados | Subtração com wrapping_sub em u32, sempre os dois do mesmo relógio |
| Vídeo, reunião ou apresentação sem tocar no teclado | Conta como ocioso e some do relatório | SHQueryUserNotificationState: com QUNS_BUSY, QUNS_PRESENTATION_MODE ou QUNS_RUNNING_D3D_FULL_SCREEN, não pausar; opção nas configurações |
| Software que injeta input (mouse jiggler, acesso remoto) | Nunca fica ocioso | Aceito; documentar |
| Bloqueio de tela, troca rápida de usuário, RDP desconectado | O foco some sem evento de foco | WTSRegisterSessionNotification → WTS_SESSION_LOCK, WTS_CONSOLE_DISCONNECT, WTS_REMOTE_DISCONNECT fecham a execução; UNLOCK e CONNECT ressincronizam |
| Suspensão e hibernação clássicas | PBT_APMSUSPEND chega antes | Fechar a execução e gravar; na volta (PBT_APMRESUMEAUTOMATIC) ressincronizar |
| Modern Standby (S0ix, maioria dos notebooks atuais) | O processo pode não receber PBT_APMSUSPEND e continuar rodando estrangulado | Também assinar GUID_CONSOLE_DISPLAY_STATE via RegisterPowerSettingNotification: tela desligada fecha a execução |
| Suspensão sem nenhum aviso | Uma execução atravessaria horas de máquina dormindo | Comparar QueryUnbiasedInterruptTime (não conta sono) com o relógio de parede a cada evento; diferença acima de 30 s fecha a execução no último instante conhecido |
| Usuário ou NTP muda a hora | Durações negativas ou gigantes | WM_TIMECHANGE fecha a execução e abre outra; validação fim > início no banco, como já existe |
| Troca de fuso (viagem) ou horário de verão | Dias com 23 ou 25 horas; entradas cruzando meia-noite | Armazenamento em UTC; a UI recorta por meia-noite local na hora de desenhar; testes para dias de 23 e 25 h |
| Desligar, reiniciar, logoff | O Windows dá poucos segundos | WM_QUERYENDSESSION/WM_ENDSESSION: fechar execução, gravar, fazer checkpoint do WAL; RegisterApplicationRestart para voltar após atualização do Windows |
| Timer manual rodando durante horas de sono | A entrada inclui o tempo dormindo | Na volta, se o sono passou do limite de ociosidade, notificação "Você ficou fora 2 h. Descartar esse tempo?" com ações Manter e Descartar |

## Banco de dados

O schema e as três migrações atuais são portados sem mudança, com o mesmo user_version. Assim um focus.db copiado do Mac abre no Windows, e a importação vira um botão em Configurações.

Configuração: journal_mode=WAL, synchronous=NORMAL, foreign_keys=ON, busy_timeout=2000 e PRAGMA optimize ao fechar. Uma thread dona da conexão de escrita recebe comandos por canal; a UI lê por uma segunda conexão só de leitura, o que o WAL permite sem bloqueio.

| Caso | O que acontece | Solução |
|---|---|---|
| Onde guardar | %APPDATA% (Roaming) sincroniza com perfis móveis corporativos e copiaria um banco que só cresce | %LOCALAPPDATA%\Focus\focus.db, via SHGetKnownFolderPath(FOLDERID_LocalAppData) |
| Banco em pasta de rede ou OneDrive | WAL depende de memória compartilhada e não funciona em SMB | Recusar caminhos de rede ao importar ou mover; avisar |
| Segunda instância do app | Dois escritores no mesmo arquivo | Mutex nomeado Local\Focus-<SID>; a segunda instância avisa a primeira para abrir a janela e sai |
| Consulta por intervalo fica lenta com anos de dados | start < ? AND end > ? usa só o índice de start e varre tudo antes do intervalo | Adicionar start >= from - duração_máxima, com a duração máxima de execução guardada em settings; o índice passa a cortar dos dois lados |
| Volume | ~2 a 5 mil execuções por dia de trabalho, ~1 milhão por ano | Suficiente para SQLite; medir consultas de "Tudo" com 3 anos de dados sintéticos |
| Disco cheio (SQLITE_FULL) ou erro de E/S | Escrita falha no meio do dia | Pausar o monitor, mostrar estado de erro e notificar; tentar de novo a cada 5 min |
| Arquivo travado por antivírus ou backup | SQLITE_BUSY | busy_timeout + nova tentativa; nunca derrubar o app |
| Corrupção | Queda de energia em disco ruim | PRAGMA quick_check semanal; cópia diária com VACUUM INTO em backups\, guardando 7 |
| Migração nova que falha no meio | Banco em estado intermediário | Cada migração numa transação, como já é; backup automático antes de migrar |
| Moeda padrão | O código atual não conhece o Brasil e cai para USD | Ler a moeda ISO do locale do usuário (GetLocaleInfoEx com LOCALE_SINTLSYMBOL): BRL para pt-BR |
| Desinstalar | Dados do usuário | Desinstalador pergunta se mantém %LOCALAPPDATA%\Focus |

## Sessões do Claude Code

A lógica atual (execuções separadas por 10 min de pausa, 60 s de cauda, 90 dias de histórico, sessões automatizadas ignoradas) é portada como está. Muda o gatilho: ReadDirectoryChangesW sobre a pasta avisa quando um .jsonl cresce, em vez do polling de 60 s.

| Caso | O que acontece | Solução |
|---|---|---|
| Pasta em outro lugar | CLAUDE_CONFIG_DIR muda a raiz | Respeitar a variável; padrão %USERPROFILE%\.claude\projects |
| Claude Code rodando no WSL | Os logs ficam em \\wsl.localhost\<distro>\home\<user>\.claude | Listar distros em HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss; ler por polling de 5 min, porque notificações de arquivo não são confiáveis por esse caminho; desligado por padrão |
| Travar o arquivo do Claude Code | Abrir sem compartilhamento impede o Claude de escrever, renomear ou apagar | Sempre FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE; abrir, ler, fechar |
| Linha ainda sendo escrita | A última linha não termina em \n | Consumir só até o último \n, guardando o offset em bytes, como hoje; tolerar \r\n |
| Linhas enormes (saídas de ferramentas, imagens em base64) | Megabytes por linha | Ler em blocos e extrair só os campos necessários com serde_json sobre RawValue; nunca carregar o arquivo inteiro |
| Nome do projeto | cwd vem como caminho Windows (C:\Users\...\trackerApp) | Pegar o último componente aceitando \ e / |
| Arquivo reescrito ou truncado | Tamanho menor que o offset salvo | Reler do início; duplicatas já são descartadas por ts <= lastTs |
| Rajada de escritas durante a resposta | Vários eventos por segundo | Agrupar em uma leitura a cada 15 s no máximo |
| Fila de notificações estoura | ERROR_NOTIFY_ENUM_DIR | Fazer uma varredura completa e seguir assistindo |
| Primeiro uso com 90 dias de logs (centenas de MB) | Varredura longa | Thread em THREAD_MODE_BACKGROUND_BEGIN (CPU e disco em baixa prioridade), com progresso na tela de boas-vindas |

## Interface nativa

A janela é criada só quando o usuário abre o app e destruída ao fechar, junto com o device Direct3D, a fábrica Direct2D e todos os caches. A UI é um pequeno toolkit próprio: árvore de widgets retida, layout com taffy, desenho em Direct2D e redesenho só quando algo muda, sem loop contínuo.

Peças do toolkit, em ordem de construção: janela e barra de título, superfície DirectComposition, texto com DirectWrite, árvore e layout, entrada por ponteiro e teclado, foco, acessibilidade, popups, e por fim os widgets (botão, toggle, segmented, select, campo, diálogo, popover, tabela, lista virtualizada, gráficos).

| Caso | O que acontece | Solução |
|---|---|---|
| Escala diferente por monitor (100%, 125%, 150%, 200%) | Interface borrada ou no tamanho errado ao mover a janela | Manifesto PerMonitorV2; layout em DIPs; em WM_DPICHANGED usar o retângulo sugerido e refazer layout e cache de texto |
| Tamanho de texto do Windows (Acessibilidade → Tamanho do texto) | Ignorado por apps Win32 por padrão | Ler UISettings.TextScaleFactor (WinRT) e aplicar a todas as fontes |
| Tema escuro | Fora da primeira versão (decidido) | Só tema claro, como o app atual; cores em tokens para o escuro entrar depois sem refazer os widgets |
| Alto contraste | Cores próprias ficam ilegíveis | Com SPI_GETHIGHCONTRAST ativo, usar GetSysColor, desligar Mica e sombras, reforçar bordas |
| Mica | Some com "Efeitos de transparência" desligado e em economia de bateria | DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW; nesses casos o próprio Windows troca por cor sólida |
| Barra de título própria | Perde Snap Layouts, arrastar, duplo clique, menu do sistema | WM_NCCALCSIZE para estender a área cliente; WM_NCHITTEST devolvendo HTCAPTION, HTMINBUTTON, HTMAXBUTTON e HTCLOSE (o flyout de Snap Layouts do Windows 11 depende de HTMAXBUTTON); margem extra quando maximizada |
| Leitores de tela (Narrador, NVDA) | Interface desenhada é invisível para eles | accesskit: cada widget publica papel, nome, estado e ações; o calendário expõe um grid com células e entradas |
| Navegação por teclado | Sem foco, sem Tab, sem atalhos | Ordem de foco, anel de foco visível, atalhos (Ctrl+N nova entrada, Ctrl+Espaço timer, setas no calendário, Esc cancela arrasto) |
| Texto, IME, colar e desfazer | Editor próprio é um projeto inteiro | Controles RICHEDIT50W filhos, sem borda, posicionados sobre o canvas; o canvas desenha a borda; corretor ortográfico com IMF_SPELLCHECKING |
| Arrastar no calendário | Precisa captura, autoscroll e snap | SetCapture; autoscroll perto das bordas; snap de 5 e 15 min; Alt desliga o snap; Esc desfaz; arrastar para outro dia |
| Tela sensível ao toque e caneta | Comum em notebooks Windows | Usar mensagens WM_POINTER* para mouse, toque e caneta num só caminho; toque longo inicia arrasto |
| Touchpad de precisão | Rolagem com deltas fracionários e horizontal | Acumular WM_MOUSEWHEEL e WM_MOUSEHWHEEL com delta fracionário |
| Perda do device gráfico | Atualização de driver, troca de GPU em notebook híbrido, RDP | Tratar D2DERR_RECREATE_TARGET e DXGI_ERROR_DEVICE_REMOVED recriando tudo |
| Janela minimizada ou coberta | Redesenho inútil | Parar o relógio do timer e qualquer animação com SIZE_MINIMIZED ou oclusão |
| Animações | Custo e preferência do usuário | Poucas, via DirectComposition; nenhuma se SPI_GETCLIENTAREAANIMATION estiver desligado |
| Popups (select, popover, seletor de data) perto da borda da tela | Cortados ou no monitor errado | MonitorFromPoint + área de trabalho do monitor para reposicionar; fechar ao perder ativação |
| Posição salva num monitor desconectado | Janela abre fora da tela | MonitorFromRect(MONITOR_DEFAULTTONULL) antes de restaurar WINDOWPLACEMENT |
| Fechar a janela | No Windows, X costuma encerrar o app | Fechar mantém o Focus na bandeja; na primeira vez, notificação explicando; "Sair" no menu da bandeja encerra |
| Primeiro dia da semana e formato de hora | Brasil começa no domingo e usa 24 h; EUA usa 12 h | LOCALE_IFIRSTDAYOFWEEK e LOCALE_STIMEFORMAT, com opção de sobrescrever |
| Idioma | pt-BR fora da primeira versão (decidido); interface em inglês, como hoje | Todas as strings num único módulo, para traduzir depois sem caçar texto no código |
| Exportar CSV para Excel em pt-BR | Excel brasileiro espera ; e vírgula decimal; sem BOM, acentos quebram | UTF-8 com BOM; separador de LOCALE_SLIST e decimal do locale; opção "CSV padrão (vírgula)" |

## Tray e timer

A bandeja do Windows não mostra texto, então o "1:23" da barra de menus do Mac não tem equivalente direto. O tempo decorrido aparece em três lugares: no tooltip do ícone, num flyout aberto pelo clique esquerdo e, opcionalmente, num mini timer flutuante.

- Ícone com estado: parado, rodando e pausado por ociosidade. Rodando, o ícone ganha um anel que completa a cada hora, redesenhado no máximo uma vez por minuto.
- Clique esquerdo: flyout no estilo Windows 11 acima da bandeja, com descrição, projeto, iniciar e parar.
- Clique direito: menu de contexto com iniciar em um projeto recente, abrir e sair.
- Mini timer opcional: janela pequena sempre no topo, arrastável, para quem quer ver o tempo o tempo todo.

| Caso | O que acontece | Solução |
|---|---|---|
| Explorer reinicia ou trava | O ícone some da bandeja | Registrar a mensagem TaskbarCreated e recriar o ícone ao recebê-la |
| Windows 11 esconde ícones novos no overflow | O usuário não vê o timer | Não há API para fixar; a tela de boas-vindas mostra como arrastar o ícone para a barra |
| Identidade do ícone por GUID | O GUID fica preso ao caminho do exe; mudar o caminho faz Shell_NotifyIcon falhar | Usar uID em vez de GUID, ou remover e recriar em caso de erro |
| Menu de contexto não fecha ao clicar fora | Bug clássico de TrackPopupMenu | SetForegroundWindow antes e PostMessage(WM_NULL) depois |
| Escala do ícone | 16 px a 100% vira 32 px a 200% | .ico com 16, 20, 24, 32, 40 e 48 px; ícone dinâmico desenhado no tamanho de GetSystemMetricsForDpi(SM_CXSMICON) |
| Barra de tarefas clara ou escura | O tema da barra é independente do tema dos apps | Ler SystemUsesLightTheme e trocar a variante do ícone em WM_SETTINGCHANGE |
| Tooltip | Limite de 128 caracteres | Truncar descrição e projeto |
| Notificações (ociosidade, erros) | Toasts de apps sem pacote exigem AUMID, atalho no Iniciar e ativador COM | Usar o balão do próprio ícone (NIF_INFO), que o Windows 11 já mostra como toast; toasts com botões ficam para depois, via um atalho no menu Iniciar criado pelo script de instalação |
| Atalho global para iniciar e parar | Pode colidir com outro app | RegisterHotKey opcional; se falhar, avisar e deixar o usuário escolher outro |
| Modo foco ou "Não perturbe" | Notificações suprimidas | Aceito; o estado continua visível no flyout |

## Build e instalação neste computador

Como o app roda só nesta máquina, não há instalador, assinatura nem canal de atualização. O entregável é um Focus.exe compilado aqui, copiado para %LOCALAPPDATA%\Programs\Focus e iniciado com o Windows.

Estado da máquina: omitido neste export (repositório público).

| Caso | O que acontece | Solução |
|---|---|---|
| Assinatura de código | Não é necessária: o Smart App Control está desligado e não há política de controle de aplicativos | Nenhuma; se o Smart App Control for ligado um dia, o exe sem assinatura passa a ser bloqueado e aí vale um certificado |
| Sophos Intercept X sinaliza o exe | Exe sem assinatura, recém-compilado, que lê títulos de janelas e inicia com o Windows parece spyware para heurísticas comportamentais | Testar já no spike (F0) com o exe rodando 8 h; nunca usar SetWindowsHookEx, Raw Input global nem UPX; se for bloqueado, pedir à TI uma exceção pelo caminho do exe |
| Ferramentas de build | Rust e as Build Tools não estão instalados; as Build Tools podem exigir administrador | Instalar rustup (por usuário, sem admin) e o Visual Studio Build Tools 2022 com a carga C++ e o Windows SDK; se faltar permissão, pedir à TI |
| Instalar e atualizar | O Windows não deixa sobrescrever um exe em uso | Script install.ps1: avisa o Focus pela janela de mensagens para sair, copia o exe novo, recria a chave Run e reabre o app |
| Início com o Windows | O usuário pode desligar pelo Gerenciador de Tarefas | HKCU\...\CurrentVersion\Run com --background (sobe direto na bandeja); ler a chave StartupApproved\Run para mostrar o estado real em Configurações |
| Diagnóstico de falhas | Crash sem pista | Panic hook + MiniDumpWriteDump em %LOCALAPPDATA%\Focus\crashes; log rotativo de 1 MB |
| Dados ao reinstalar | Apagar a pasta do programa não pode levar o banco junto | Programa em %LOCALAPPDATA%\Programs\Focus, dados em %LOCALAPPDATA%\Focus, sempre separados |

## Orçamento de recursos e como medir

Cada meta abaixo vira um passo de um roteiro de verificação local; uma versão que estoura o orçamento não é instalada. Os valores são metas de projeto, a confirmar no spike.

| Métrica | Meta | Como medir |
|---|---|---|
| Memória privada, janela fechada | < 5 MB | PrivateMemorySize64 a cada minuto num teste de 8 h; a curva tem que ser plana (sem vazamento) |
| Working set, janela fechada | < 10 MB | Mesmo teste |
| CPU média, janela fechada | < 0,05% | WPR com perfil CPU Usage (Precise) durante 1 h de uso normal |
| Wakeups da thread, janela fechada | < 1 por segundo em média | WPA: troca de contexto por thread; Process Explorer "Context switch delta" |
| Escrita em disco | < 1 MB por hora | Contadores de E/S do processo no teste de 8 h |
| Janela aberta, parada | < 60 MB working set; 0% CPU sem animação | Abrir as 6 telas, esperar 1 min, medir |
| Arrastar no calendário | 60 quadros por segundo a 4K e 200% | Contador de quadros em build de diagnóstico |
| Memória após fechar a janela | Fica < 25 MB privados e plana (decisão 0004: a pilha gráfica retém ~21 MB após a primeira abertura) | Abrir e fechar a janela 50 vezes no teste automatizado |
| Partida até a bandeja | < 150 ms | Timestamp no log |
| Abrir a janela (fria) | < 200 ms até o primeiro quadro | Timestamp no log |
| Tamanho do binário | < 5 MB | O script de build avisa acima disso |

Técnicas que sustentam essas metas:

- EcoQoS: as threads de monitor, Claude e banco rodam com ThreadPowerThrottling ligado, o que no Windows 11 as leva para núcleos de eficiência. A thread de UI desliga o estrangulamento só enquanto a janela está aberta.
- Prioridade de memória baixa (MEMORY_PRIORITY_LOW) em segundo plano, para o Windows reaproveitar essas páginas primeiro quando faltar memória.
- Timers coalescíveis em tudo que é periódico, para o processador dormir junto com o resto do sistema.
- Sem truques de métrica: nada de EmptyWorkingSet periódico; ele só esconde o número e gera page faults.

## Fases e marcos

O Gate 0 é o ponto de decisão: se o spike não bater as metas de memória e CPU, ou se a UI Automation pesar no navegador, o plano volta para Tauri antes de qualquer investimento em interface.

## Controle de versão e rastreio

Toda alteração vira um commit pequeno no repositório focus-app; nada fica só na máquina. O código Rust fica numa pasta win32/ no mesmo repositório, e o app Electron continua lá, intacto, como referência de comportamento até o fim da F5.

- Cada fase tem uma branch saindo da main: win32/f0-spike, win32/f1-nucleo, win32/f2-base-ui, win32/f3-telas, win32/f4-build, win32/f5-uso.
- Commit a cada mudança que compila e passa nos testes, nunca um dia inteiro num commit só. Trabalho incompleto no fim do dia pode ir com WIP: no assunto, só na branch da fase.
- Push para o origin ao fim de cada sessão de trabalho, para o histórico existir fora desta máquina.
- Ao passar um gate: merge na main sem squash (cada passo continua visível) e tag win32-gate-0 a win32-gate-4.

| Regra | Como |
|---|---|
| Mensagem | Inglês no imperativo, como o histórico atual ("Add foreground window hook"); o corpo explica o porquê e, quando houver, a medição antes e depois |
| Autoria | Commits feitos pelo Claude terminam com Co-Authored-By: Claude, então dá para separar o que foi escrito pelo agente e o que foi escrito à mão |
| Verificação antes do commit | Hook em .githooks/pre-commit (ativado com git config core.hooksPath .githooks) rodando cargo fmt --check, cargo clippy -- -D warnings e cargo test |
| Medições | Resultado de cada teste de 8 h e de cada gate em win32/docs/measurements/AAAA-MM-DD-<teste>.csv, commitado: o consumo de memória e CPU ganha histórico |
| Decisões | Cada decisão de arquitetura num arquivo curto win32/docs/decisions/NNNN-<titulo>.md, a começar pelas seis decisões de escopo deste plano |
| O próprio plano | Este doc não está no Git; a cada gate, exportar em Markdown para win32/docs/plan.md e commitar, para o plano também ter histórico |
| Versão | version do Cargo.toml em 0.x, subindo a cada gate junto com a tag; Cargo.lock sempre commitado |
| Fins de linha | .gitattributes com * text=auto e *.rs text eol=lf, para o autocrlf do Git no Windows não gerar diffs falsos |

O que nunca entra no repositório, via .gitignore: win32/target/, Focus.exe, *.db, *.db-wal, *.db-shm, *.dmp e a pasta crashes/. O banco, os backups e os dumps de memória contêm títulos de janelas e URLs reais; os testes usam bancos em memória e dados sintéticos. Se o repositório no GitHub for público, isso é ainda mais importante.

## Riscos principais

O maior risco é de prazo, não técnico: a interface ocupa 9 a 12 das 16 a 20 semanas e não tem atalho em Win32 puro.

| Risco | Impacto | Mitigação |
|---|---|---|
| Toolkit de UI próprio cresce além do previsto | Alto: atrasa todas as telas | Fechar a lista de widgets antes da fase de telas; nada de widget genérico sem tela que o use |
| UI Automation pesa no Chrome ou no Edge | Médio: o Focus deixaria o navegador mais lento, o oposto do objetivo | Medir no spike; se pesar, URL desligada e só o título da aba |
| Sophos bloqueia ou coloca o exe em quarentena | Alto: o app para de rodar sem aviso | Testar no spike; pedir exceção à TI se preciso |
| Sem permissão para instalar as Build Tools | Alto: não dá para compilar | Resolver com a TI antes do spike |
| Acessibilidade fica para o fim | Médio: retrabalho em todos os widgets | accesskit entra junto com o primeiro widget; Narrador testado a cada tela |
| Pouca experiência com Rust e Win32 | Médio: ritmo lento nas primeiras semanas | O spike serve também de treino; revisão de código focada em unsafe |

## Decisões de escopo

Respondidas em ; o plano acima já reflete cada uma.

| Pergunta | Decisão | O que mudou no plano |
|---|---|---|
| A versão Mac continua? | Só a versão Windows | Sai o risco de duas bases de código e a importação do banco do Mac; o schema continua o mesmo porque não custa nada |
| Modo escuro e pt-BR na primeira versão? | Não | Interface clara e em inglês, como hoje; strings e cores centralizadas para depois |
| Windows 10 ou só Windows 11? | Só Windows 11 | Saem os fallbacks de Mica, ícones e fontes |
| Quem assina o código? | O próprio usuário, sem Azure Artifact Signing | Assinatura deixa de ser necessária, porque o app roda só aqui e o Smart App Control está desligado |
| Onde o app vai rodar? | Só neste computador | Sem instalador, winget, MSI nem ARM64; build local e um script de instalação |
| Extensão de navegador? | Não | URL só por UI Automation; se ela pesar, só o título da aba |

Pergunta nova: as Build Tools do Visual Studio podem exigir administrador nesta máquina. Se for o caso, a TI precisa instalar antes do spike.
