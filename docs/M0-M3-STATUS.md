# M0–M3 status

Дата: 2026-11-24.  
Публичная версия Rustle: 0.42.0.  
Текущая ветка: M0 + M1 + M3 static, M2 в процессе.

## M0. Baseline

Сделано:

- добавлен воспроизводимый benchmark harness;
- добавлены feature-флаги-константы и контракт StaticRenderer;
- зафиксированы версии движков: Obscura 0.2.7, Servo 0.8.0;
- добавлен скрипт загрузки движков и таблица метрик.

Первый прогон (Windows, один запуск, без прогрева):

| Движок | URL | Render, мс | PNG, байт |
|---|---|---:|---:|
| Obscura | https://example.com | 639 | 63 715 |
| Obscura | https://www.rust-lang.org | 2 299 | 73 317 |
| Obscura | https://servo.org | 3 153 | 125 802 |
| Servo | https://example.com | 981 | 109 719 |
| Servo | https://www.rust-lang.org | 1 538 | 77 172 |
| Servo | https://servo.org | 2 037 | 160 162 |

Это не финальные KPI: нужны прогревы, несколько повторов, RSS и p50/p95.

Примеры статического рендера:

- [Obscura](assets/engines/obscura-example.png)
- [Servo](assets/engines/servo-example.png)

## M1. Obscura spike

Сделано:

- crate rustle-obscura;
- поиск sidecar по RUSTLE_OBSCURA_BIN, рядом с exe или в tools/obscura;
- запуск obscura fetch в отдельном процессе;
- screenshot и text capture;
- таймауты и kill процесса;
- проверка PNG-сигнатуры.

Ограничение: это sidecar, а не встроенное browser core. Следующий шаг — CDP-сессия и вкладки внутри Rustle UI.

## M2. Obscura alpha (CDP-мост)

Сделано:

- запуск Obscura serve на свободном порту;
- ожидание /json/version и WebSocket CDP;
- Target.createTarget / attachToTarget;
- Page.enable / navigate / reload;
- screenshot через Page.captureScreenshot;
- Input.dispatchMouseEvent и dispatchKeyEvent;
- браузерные контексты Obscura для профилей;
- **нативные вкладки Rustle**: каждый Tab с engine=obscura получает свой Target и поверхность surface.html;
- адресная строка Rustle работает с native-вкладкой;
- переключение вкладок активирует нужный Target и обновляет кадр;
- закрытие вкладки закрывает Target;
- back/forward через Page.getNavigationHistory и Page.navigateToHistoryEntry;
- обновление кадра опросом surface каждые 900 мс;
- сетевой трекер-блок Obscura в режиме --stealth (3520 доменов);
- настройка «Движок новых вкладок»: WebView2 / Obscura (CDP);
- opt-in через настройки;
- дополнительный viewer с вкладками и профилями для отладки.

Ограничения:

- Adblock Rustle пока не применяет собственные правила к NetworkService: используется встроенный трекер-блок Obscura;
- кадр обновляется после команд, а не live-потоком;
- профильные контексты живут в памяти процесса;
- сложные сайты могут отличаться от WebView2.

[Скриншот нативных вкладок](assets/engines/obscura-native-tabs.png)

## M3. Servo static renderer

Сделано:

- crate rustle-servo;
- запуск servoshell -z -x -o PNG;
- viewport size, timeout, kill;
- PNG validation;
- CLI rustle-capture для обоих движков;
- сравнение статического рендера Obscura/Servo на одном URL-наборе.

Ограничение: это static PNG renderer, а не встроенный интерактивный compositor. Следующий шаг — renderer-процесс с shared-memory surface и вводом.

## Артефакты

- crates/rustle-engine — контракт StaticRenderer;
- crates/rustle-obscura — Obscura sidecar;
- crates/rustle-servo — Servo static sidecar;
- crates/rustle-engine-cli — rustle-capture;
- scripts/fetch-engines.ps1;
- scripts/bench-engines.ps1;
- tools/engines.json.

Бинарники движков не коммитятся: они скачиваются скриптом.

## M4. Интерактивный Servo (spike)

Сделано:

- запуск servoshell как дочернего процесса без консольного окна;
- встраивание HWND Servo в окно Rustle через SetParent;
- обрезка собственного toolbar Servo, чтобы остался только контент;
- нативные вкладки Rustle с отдельным Servo-процессом на вкладку;
- переключение/закрытие вкладок;
- адресная строка Rustle перезапускает Servo на новый URL;
- профильный config-dir Servo;
- back/forward по URL-истории вкладки с перезапуском Servo;
- нативные стрелки Rustle справа от домика;
- опция «Движок новых вкладок → Servo (встроенное окно)».

Ограничения:

- это HWND-встраивание, а не shared-memory renderer-процесс;
- back/forward перезапускает Servo, а не листает внутреннюю историю;
- заголовок и favicon берутся из URL, потому что servoshell отдаёт только общий title;
- нет общей блокировки рекламы на уровне Rustle;
- полноценный compositor/input protocol — следующий шаг.

[Скриншот встроенного Servo](assets/engines/servo-embedded.png)

## M5. Servo parity и параллелизм (начато)

Сделано:

- Servo запускается с --layout-threads (половина доступных ядер, 1–8);
- каждая Servo-вкладка — отдельный процесс;
- бюджет одновременных Servo-процессов: 8, байтовый budget 1600 МБ, неактивные выгружаются по LRU;
- выгруженная вкладка восстанавливается при активации;
- benchmark: scripts/bench-servo-tabs.ps1 считает startup, RSS Rustle и Servo;
- замеры: при 8 процессах RSS Servo растёт примерно до 1,6 ГБ; старые замеры 3 процессов больше не актуальны.

Ограничения:

- это процессный параллелизм, а не управляемый work-stealing внутри Servo;
- память ограничена количеством процессов, не байтовым budget;
- нет p95 input latency и top-1000 parity;
- WebRender scene/raster и decode вне UI-потока ещё не настраиваются из Rustle.

## M6. Hardening

Сделано:

- автоматический WebView2 fallback, если Servo не найден или не стартовал;
- crash log в %LOCALAPPDATA%\Rustle\crashes.log;
- отдельный процесс на Servo-вкладку и LRU-выгрузка;
- CI: fmt, clippy, test, release build;
- миграция профилей и настроек без потери данных.

Ограничения:

- телеметрия не отправляется;
- нет отдельного sandbox-профиля и site isolation за пределами процесса вкладки;
- security review не завершён.

## M7. Rustle 1.0

Сделано:

- версия 1.0.0;
- Servo входит в portable/setup;
- WebView2 выбран движком по умолчанию для совместимости; Servo доступен opt-in;
- WebView2 fallback при отсутствии/падении Servo;
- документация Known Issues в release notes;
- публичные артефакты setup/portable.

Известные ограничения 1.0:

- Servo-вкладки перезапускаются при смене URL/back/forward;
- по умолчанию WebView2; Servo opt-in и до 8 живых процессов;
- favicon/точный title Servo ограничены;
- нет top-1000 parity и p95 input latency.
