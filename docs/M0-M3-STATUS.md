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

## M2. Obscura alpha

В работе. Уже есть:

- процессный adapter;
- статический capture;
- изолированный таймаут.

Ещё нет:

- интерактивных вкладок через Obscura;
- профилей/cookie-store внутри Obscura;
- back/forward и истории;
- Adblock на NetworkService;
- opt-in переключателя в UI.

Без этих пунктов нельзя считать M2 завершённым.

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
