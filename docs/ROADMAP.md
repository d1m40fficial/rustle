# Rustle — roadmap

Текущая публичная версия: 0.42.0.  
Стратегическое направление: Rustle UI + Obscura browser core + Servo renderer.

## Принципы

- Сначала измеряем, потом оптимизируем.
- Новый движок включается только через feature flag.
- WebView2 остаётся fallback до подтверждённой совместимости.
- Каждый релиз имеет измеримые exit criteria.
- Память и input latency важнее синтетических бенчмарков.
- Никаких big-bang переключений.

## M0. Базовая линия (0.43)

Цель: зафиксировать текущие показатели Rustle/WebView2.

Deliverables:

- benchmark harness: старт, новая вкладка, input latency, RAM 1/5/10/30 вкладок;
- набор 100–200 сайтов для page load;
- ночной отчёт и графики;
- ADR: лицензии Obscura/Servo, границы интеграции, риски;
- фича-флаги engine и renderer;
- критерии автоматического отката.

Exit:

- метрики воспроизводимы;
- есть baseline-таблица;
- текущие функции не сломаны.

## M1. Obscura spike (0.44)

Цель: доказать, что Obscura можно подключить как browser core.

Deliverables:

- вендор Obscura на фиксированном tag/commit;
- BrowserEngine trait и адаптер WebView2;
- Obscura открывает одну вкладку в отдельном процессе;
- навигация, title, loading/error, favicon;
- feature flag obscura, скрытый по умолчанию.

Exit:

- одна вкладка открывается и закрывается без падения UI;
- краш renderer не роняет Rustle;
- замеры старта и RAM прототипа.

## M2. Obscura alpha (0.45)

Цель: повседневная навигация на ограниченном наборе сайтов.

Deliverables:

- вкладки, история, back/forward, reload, stop;
- профили и отдельные cookie/store;
- загрузки;
- Adblock через NetworkService;
- сессии и pinned tabs;
- opt-in включение для тестового профиля;
- автоматический fallback на WebView2.

Exit:

- 95% топ-1000 без блокирующих ошибок;
- новая вкладка < 150 мс;
- RAM на 10 вкладок не выше WebView2;
- crash-free > 99.5% в тесте.

## M3. Servo static renderer (0.46)

Цель: заменить renderer Obscura на Servo для статических страниц.

Deliverables:

- Servo в отдельном renderer-процессе;
- shared-memory surface, resize, scroll, screenshot;
- HTML/CSS/DOM, style, layout, paint;
- pixel-diff и layout-diff против WebView2;
- feature flag renderer = servo.

Exit:

- 90% тестового набора без визуальных блокеров;
- input latency p95 < 80 мс;
- память не выше Obscura renderer на том же наборе.

## M4. Servo interactive alpha (0.47)

Цель: интерактивные страницы на Servo.

Deliverables:

- JS runtime и event loop;
- формы, клавиатура, clipboard, selection, IME;
- canvas, базовые media;
- focus и accessibility tree;
- devtools-хуки.

Exit:

- логины и формы на 90% тестового набора;
- input latency p95 < 60 мс;
- нет утечек памяти за 2 часа;
- crash-free > 99.5%.

## M5. Servo parity и параллелизм (0.48)

Цель: Servo-путь не хуже WebView2 на большинстве сайтов.

Deliverables:

- параллельные style/layout по независимым DOM-поддеревьям;
- WebRender scene building и raster;
- image decode, font shaping и compositing вне UI-потока;
- work-stealing scheduler и приоритеты input/visible/prefetch;
- memory governor и выгрузка фоновых вкладок;
- стресс-тесты 10/30/50 вкладок.

Exit:

- top-1000 parity > 95%;
- input latency p95 < 50 мс;
- RAM 10 вкладок < 700 МБ;
- RAM 30 вкладок <= WebView2 или -20%;
- crash-free > 99.9%.

## M6. Hardening и подготовка к default (0.49)

Deliverables:

- sandbox и site isolation;
- TLS и обновления зависимостей;
- crash reporter;
- телеметрия opt-in;
- regression gates в CI;
- миграция профилей и настроек без потерь.

Exit:

- security review;
- две недели dogfooding без блокирующих багов;
- автоматический fallback работает.

## M7. Rustle 1.0 на Servo (1.0)

Цель: Servo-путь по умолчанию для большинства сайтов.

Deliverables:

- renderer = servo по умолчанию;
- WebView2 fallback для DRM и несовместимых сайтов;
- документация и Known Issues;
- публичный релиз.

Exit:

- KPI выполнены;
- нет регрессий профилей, истории и загрузок;
- пользователь может вручную вернуть WebView2.

## После 1.0

- Site isolation и process-per-site.
- Профили с разными движками.
- WebExtensions или собственный API.
- Синхронизация с E2E-шифрованием.
- Tree tabs, split-screen, PiP.
- Встроенный перевод, reader mode.
- Мобильные платформы — отдельное решение.

## Текущие продуктовые направления (не блокируют M0–M7)

- Полировка UI: меню, настройки, панели, доступность.
- История, закладки, сессии: производительность и UX.
- Загрузки: очередь, докачка, лимиты.
- Adblock: полный ABP-синтаксис, импорт списков.
- Локализация ru/en без хардкода.
- Crash reporter и диагностика.

## Метрики релиза

| Метрика | Цель |
|---|---|
| Cold start p50 | < 800 мс |
| Новая вкладка p50 | < 120 мс |
| Input latency p95 | < 50 мс |
| RAM 1 вкладка | < 120 МБ |
| RAM 10 вкладок | < 700 МБ |
| RAM 30 вкладок | <= WebView2 или -20% |
| Page load p75 | -20% |
| Top-1000 compatibility | > 95% |
| Crash-free sessions | > 99.9% |
