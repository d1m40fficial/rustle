# M0: отчёт о возможностях движка

**Дата:** 2026-11-06
**Среда:** Windows 11 IoT Enterprise LTSC 26100, WebView2 Runtime 156.0.4314.58,
Rust 1.99.1 (x86_64-pc-windows-gnu), MinGW-w64 16.2.0, wry 0.57.1, tao 0.37.2.

Отчёт фиксирует, что проверено на реальном прототипе Rustle M0, что не проверено
и какие решения приняты до начала M1.

## Итог

Архитектура жизнеспособна. Все критичные для M1 механизмы работают:

- несколько дочерних WebView в одном окне;
- кастомный протокол `rustle://` с HTML/CSS/JS и строгим CSP;
- IPC между оболочкой и Rust;
- z-order: chrome WebView перекрывает content WebView при растягивании;
- перехват Ctrl+L при фокусе в контенте через WebView2 AcceleratorKeyPressed;
- ввод с клавиатуры и навигация по адресной строке.

Блокеров для M1 нет. Два вопроса требуют решения: бюджет памяти и способ
поставки WebView2Loader.dll для GNU-сборки (решён вендорингом).

## Матрица проверок

| Возможность | Статус | Детали |
|---|---|---|
| Несколько дочерних WebView | работает | Два `WRY_WEBVIEW` в одном окне tao: chrome 78 px и content ниже |
| Bounds и resize | работает | `set_bounds` при старте и на `WindowEvent::Resized` |
| Z-order chrome поверх content | работает | content создаётся первым, chrome вторым; chrome растягивается до 360 px и перекрывает страницу |
| Custom protocol + ES-модули | работает | `rustle://localhost/chrome/index.html`, MIME и CSP отдаются вручную |
| CSP | работает | `script-src 'self'`; inline-скриптов нет, внешние модули грузятся |
| IPC chrome → Rust | работает | `chrome.ready` приходит на 217 мс (debug), `nav.go` открывает сайт |
| IPC Rust → JS | работает | `app.info` обновляет бейдж версии в оболочке |
| Клавиатура при фокусе в контенте | работает | AcceleratorKeyPressed + `with_browser_accelerator_keys(false)`; Ctrl+L фокусирует адресную строку |
| Key down / key up | работает | Событие приходит дважды; обрабатывается только `KEY_DOWN`, иначе команда выполнится дважды |
| WebView2Loader.dll | требуется | GNU-сборка импортирует DLL; вендорена в `assets/webview2/x64`, build.rs копирует рядом с exe |
| Окно и первый кадр | работает | окно 10–12 мс, content 152–156 мс, chrome 198–203 мс, chrome-ready 214–217 мс (debug) |
| Память | замерена | 437 МБ на 9 процессов: chrome + content с example.com, debug, включая conhost 13.6 МБ |
| Downloads | не проверено | нужен DownloadStarting API через webview2-com |
| Permissions | не проверено | нужен PermissionRequested |
| Zoom / Find | не проверено | WebView2 ZoomFactor и Find API |
| InPrivate / профиль | не проверено | CoreWebView2Profile.IsInPrivateModeEnabled или временный каталог |
| DevTools | не проверено | открытие через controller |
| PDF / YouTube | не проверено | smoke-тест M1 |
| Падение рендерера | не проверено | ProcessFailed |

## Клавиатура: важные детали

WebView2 по умолчанию сам обрабатывает часть браузерных клавиш. Чтобы получать их
в приложении, у обоих WebView выставлено:

```rust
.with_browser_accelerator_keys(false)
```

Затем на контроллеры вешается `AcceleratorKeyPressed` (webview2-com). Найдено:

- событие приходит на key down и key up; обрабатывать только `COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN`;
- при синтетическом вводе через `SendKeys` поле ScanCode равно 0, а буква может
  приходить как `VK_PROCESSKEY` (0xE7) после IME;
- при вводе через `keybd_event` с реальным scan code приходит `VK=0x4C`,
  `ScanCode=0x26`, и Ctrl+L срабатывает;
- для раскладок, отличных от US, надёжнее ориентироваться на физический ScanCode,
  а не только на виртуальную клавишу.

Итог: Ctrl+L из контента фокусирует адресную строку, дальнейший ввод и Enter
уходят в `nav.go` и открывают сайт. Механизм пригоден для Ctrl+T/W/F и остальных
горячих клавиш в M1–M2.

## Память

Сумма WorkingSet по процессам переоценивает потребление: общие страницы DLL и
shared memory учитываются в каждом процессе. Поэтому фиксируем два числа —
private bytes (ближе к колонке «Память» в Task Manager) и сумму WorkingSet
(верхняя оценка).

| Сценарий | Private | Working set | Процессов |
|---|---|---|---|
| Новая вкладка, debug | 186.4 МБ | 402.2 МБ | 9 |
| Новая вкладка, release | 172.0 МБ | 377.7 МБ | 9 |
| example.com, debug | 190.0 МБ | 424.9 МБ | 9 |

Цель ТЗ «1 пустая вкладка ≤ 250 МБ» **выполняется** по private bytes: 172–186 МБ.
Сумма WorkingSet выше 400 МБ из-за общих страниц и не должна использоваться как
единственная метрика. Для отчётов фиксируем private, WorkingSet — справочно.

Память можно улучшить memory saver'ом после 0.1, но архитектурного блокера нет.

## Что решено

- ADR-0001: WebView2 + wry — движок 0.1.
- ADR-0002: HTML-оболочка в отдельном WebView с растягиванием для оверлеев.
- ADR-0003: GNU-цепочка через Scoop + вендоринг WebView2Loader.dll.

## Следующие шаги

- M1: навигация, адресная строка, ошибки, синхронизация URL/заголовка.
- Проверить downloads, permissions, zoom, find, InPrivate, PDF/YouTube.
- Снять release-метрики старта и памяти, обновить бюджет.