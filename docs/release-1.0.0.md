# Rustle 1.0.0

Релиз с WebView2 по умолчанию и экспериментальным Servo-движком.

## Новое

- WebView2 по умолчанию для совместимости.
- Servo-вкладки: встроенный интерактивный Servo, отдельный процесс на вкладку (opt-in).
- WebView2 fallback, если Servo недоступен.
- Obscura CDP как альтернативный движок.
- Нативные стрелки Назад/Вперёд справа от домика.
- Back/forward для Servo через URL-историю вкладки.
- Бюджет памяти Servo: 650 МБ, LRU-выгрузка неактивных вкладок.
- Параллельный layout Servo: 1–8 потоков.
- Crash log: %LOCALAPPDATA%\Rustle\crashes.log.
- Без консольных окон у Rustle, Obscura и Servo.

## Известные ограничения

- Servo-вкладки восстанавливаются при активации после выгрузки.
- Back/forward Servo перезапускает процесс.
- Нет top-1000 parity и p95 input latency.
- Favicon Servo берётся из URL.
