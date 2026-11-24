# План развития Rustle: Obscura + Servo

## 1. Цель

Перевести Rustle с текущего WebView2-стека на собственный Rust-браузерный стек:

1. **Obscura** — основа браузера: окно, вкладки, профили, сеть, кэш, планировщик, память, загрузки, безопасность.
2. **Servo** — зрелый параллельный renderer внутри Obscura: DOM/CSS/layout/paint/compositing вместо текущего компонента рендеринга Obscura.

Ключевые приоритеты:

- скорость отклика интерфейса;
- быстрый старт и открытие вкладок;
- минимальный расход RAM;
- многоядерная загрузка и render;
- совместимость с современными сайтами;
- предсказуемый fallback на WebView2 там, где новый стек ещё не готов.

## 2. Текущее состояние

- Rustle UI: Rust + tao + wry + WebView2.
- Работают вкладки, профили, история, закладки, загрузки, Adblock, сессии и настройки.
- WebView2 остаётся референсным и fallback-движком на время миграции.
- Ограничения текущего стека:
  - память на вкладку определяется WebView2;
  - ограниченный контроль над сетевым стеком и кэшем;
  - нельзя глубоко управлять параллелизмом render;
  - поведение зависит от Edge Runtime и политик Windows.

## 3. Целевая архитектура

    Rustle UI (tao + wry chrome/sidebar)
        |
        | Browser API (tabs, profiles, history, downloads, settings)
        v
    Obscura browser core (Rust)
        |- session/profile manager
        |- network: HTTP/2/3, cookies, cache, TLS
        |- tab scheduler and process manager
        |- memory governor
        |- downloads and permissions
        |- renderer abstraction
             |- Obscura renderer (temporary)
             +- Servo renderer (target)
                   |- HTML parser / DOM
                   |- style + layout (parallel)
                   |- paint + WebRender
                   |- compositor / raster / image decode
        |
        v
    Shared-memory IPC + crash-isolated renderer processes

### 3.1. Границы ответственности

- **Rustle UI** не знает, какой renderer используется. Он общается с BrowserEngine trait.
- **Obscura** владеет навигацией, сетью, cookie/profile, кэшем, загрузками, политиками и жизненным циклом вкладок.
- **Servo** владеет DOM, style, layout, paint и композитингом страницы.
- **WebView2** — отдельная реализация BrowserEngine для fallback и сравнения метрик.

### 3.2. Ключевые интерфейсы

- BrowserEngine: create/close tab, navigate, reload, back/forward, zoom, find, print/PDF, permissions.
- RendererBackend: load document, resize viewport, input events, focus/IME, accessibility tree, screenshot.
- SessionStore: tabs, groups, pinned tabs, profile-scoped session.
- NetworkService: request interception для Adblock, cache, proxy, cookies.
- MemoryGovernor: budget per process/tab, pressure levels, discard policy.

## 4. Этапы

### Этап 0. Базовая линия и guardrails

Задачи:

- Зафиксировать метрики текущего Rustle/WebView2:
  - cold/warm start;
  - время открытия новой вкладки;
  - input latency;
  - RAM при 1/5/10/30 вкладках;
  - page load на наборе из 100–200 сайтов;
  - crash-free sessions.
- Добавить benchmark harness и ночной прогон.
- Зафиксировать лицензии Obscura, Servo и зависимостей; проверить совместимость с распространением Rustle.
- Ввести feature flags engine = webview2 | obscura и renderer = obscura | servo.
- Определить критерии отката: сайт или сценарий автоматически остаётся на WebView2.

Выход:

- воспроизводимые цифры «до»;
- ADR по интеграции;
- ни одна текущая функция не регрессирует.

### Этап 1. Внедрение Obscura

Задачи:

- Вендорить Obscura в workspace (vendor/obscura или отдельные crates) на фиксированном commit/tag.
- Адаптировать BrowserEngine:
  - вкладки, история навигации, favicon, title, loading/error;
  - профили и отдельные cookie/store;
  - загрузки и прогресс;
  - permissions;
  - zoom, find, PDF/print;
  - контекстные меню и drag-and-drop.
- Перенести Adblock на слой NetworkService Obscura.
- Подключить существующую Rustle-сессию: tabs/groups/pinned, restore, auto-suspend.
- Запустить Obscura в отдельном процессе браузера; UI-поток не блокировать.
- Сделать Obscura opt-in для ограниченного набора сайтов.
- Сравнить с WebView2 на одном и том же наборе страниц.

Целевые метрики первого этапа:

- cold start не хуже WebView2;
- новая вкладка < 150 мс;
- RAM на 10 вкладок не выше WebView2;
- 95% топ-1000 сайтов открываются без блокирующих ошибок;
- падение renderer не убивает UI.

Выход:

- Obscura проходит smoke/навигацию/профили/загрузки;
- есть автоматический откат на WebView2;
- известен список несовместимостей.

### Этап 2. Замена renderer на Servo

Цель — заменить только renderer-компонент Obscura, не переписывая UI и сеть.

Порядок:

1. **Servo как отдельный renderer-процесс**
   - shared-memory transport;
   - viewport resize, scroll, input, focus, IME;
   - screenshot и compositor surface.
2. **Статический рендеринг**
   - HTML/CSS/DOM, style, layout, paint;
   - без полноценного JS на первом шаге;
   - сравнение pixel-diff и layout-метрик с WebView2.
3. **Интерактивность**
   - JS runtime и event loop;
   - формы, клавиатура, clipboard, selection;
   - media и canvas — по мере готовности.
4. **Параллельный конвейер**
   - параллельные style/layout по независимым поддеревьям DOM;
   - WebRender scene building и raster;
   - отдельные потоки image decode, font shaping, compositing;
   - work-stealing scheduler и приоритеты: input > visible viewport > prefetch.
5. **Память**
   - бюджет на renderer-процесс и вкладку;
   - арены/пулы, освобождение невидимых документов;
   - давление памяти → выгрузка фоновых вкладок;
   - запрет неограниченного роста кэша decoded images;
   - измерения p50/p95 RSS и peak.
6. **Совместимость**
   - feature detection и fallback на Obscura renderer/WebView2;
   - DRM, WebGL, WebGPU, extensions — отдельные решения;
   - accessibility tree и devtools.

Выход:

- Servo-путь проходит top-1000 сайтов не хуже 95% parity;
- input latency p95 < 50 мс;
- RAM на 10 вкладок не выше Obscura-рендерера;
- нет регрессии crash-free.

### Этап 3. Углубление параллелизма Obscura

После подключения Servo оптимизировать весь браузер:

- сеть: параллельные DNS/TLS/HTTP-загрузки, приоритеты, coalescing;
- декодирование изображений/медиа вне UI-потока;
- кэш: sharded cache, lock-free чтение, фоновой eviction;
- JS: отдельный process/thread pool, budget на выполнение;
- вкладки: scheduler по видимости, памяти и user input;
- профили: изоляция cookie/store без блокировки UI;
- телеметрия только opt-in.

## 5. KPI

| Метрика | Цель |
|---|---|
| Cold start (p50) | < 800 мс |
| Новая вкладка (p50) | < 120 мс |
| Input latency (p95) | < 50 мс |
| RAM, 1 вкладка | < 120 МБ |
| RAM, 10 вкладок | < 700 МБ |
| RAM, 30 вкладок | < 1.5 ГБ или -30% к WebView2 |
| Page load (p75) | -20% к WebView2 |
| Crash-free sessions | > 99.9% |
| Top-1000 compatibility | > 95% |

Цифры уточняются после Этапа 0 на реальном железе.

## 6. Риски

- **Servo не является drop-in заменой Chromium**: media/DRM/расширения могут быть неполными.
- **Obscura может быть молодой кодовой базой**: нужен аудит архитектуры, безопасности и лицензий.
- **Два движка одновременно** усложняют тестирование; обязательны feature flags и автоматический fallback.
- **Параллелизм не равен скорости**: избыточные процессы повышают RAM; нужен memory governor.
- **Сайты, зависящие от Chromium-специфики**, могут требовать WebView2 ещё долго.
- **Безопасность**: sandbox, site isolation, TLS, обновления обязательны до включения по умолчанию.

## 7. Правила миграции

- Никаких big-bang переключений.
- Каждый этап имеет измеримый exit criteria.
- WebView2 остаётся доступен до стабилизации Servo-пути.
- Все новые функции Rustle сразу проектируются через BrowserEngine, без привязки к WebView2.
- Регрессии памяти и input latency блокируют релиз.

## 8. Что не входит в первую миграцию

- Полная замена UI-слоя.
- Поддержка всех WebExtensions.
- DRM-видео и стриминговые сервисы на Servo-пути.
- Мобильные платформы.
