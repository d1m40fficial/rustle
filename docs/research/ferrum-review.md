# Обзор https://github.com/aman696/browser

**Дата:** 2026-11-10  
**Проект:** Ferrum — экспериментальный браузерный движок на Rust (MPL-2.0).

## Что внутри

- Собственный workspace из 8 крейтов: net, html, css, layout, render, js, security, browser.
- Наиболее зрелый слой — сеть:
  - HTTPS-only с запретом downgrade;
  - DNS-over-HTTPS через hickory-resolver;
  - HSTS в памяти;
  - SSRF-защита (RFC 1918, loopback, CGNAT);
  - таймауты, лимит ответа, ограничение редиректов;
  - запрет userinfo/fragment/zone-id/CRLF;
  - Sec-Fetch-* и отсутствие Referer.
- HTML-токенизатор/парсер частично готов; CSS/layout/render/JS — заглушки.
- UI браузера (вкладки, адресная строка, меню) не реализован.
- Лицензия MPL-2.0, file-level copyleft.

## Вывод

Напрямую перенести наработки в Rustle нельзя: Rustle использует WebView2 и собственную оболочку,
а Ferrum — это отдельный движок без готового UI/рендеринга. Vendoring нецелесообразен.

## Что стоит взять как идеи

1. **Единый сетевой chokepoint** для политик приватности — в Rustle эту роль уже выполняет
   WebView2 + Adblock-слой; можно усилить централизованными исключениями и HTTPS-first.
2. **Строгие таймауты/лимиты/редиректы** — полезный чеклист для будущих сетевых функций.
3. **HSTS/HTTPS-only** — частично уже есть через HTTPS-first; можно расширить.
4. **Удаление tracking-параметров** (utm_*, fbclid, gclid) — кандидат в backlog.
5. **Per-site JS opt-in** — концептуально полезно, но WebView2 усложняет реализацию.

## Решение

Не добавлять код Ferrum в Rustle. Зафиксировать идеи в backlog; приоритет — стабильность
WebView2-оболочки, групп, drag-and-drop и Adblock-исключений.
