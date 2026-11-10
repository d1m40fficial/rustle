(() => {
  const form = document.getElementById("search");
  const input = document.getElementById("q");

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const query = input.value.trim();
    if (!query) {
      return;
    }

    if (window.chrome && window.chrome.webview) {
      window.chrome.webview.postMessage(JSON.stringify({ type: "nav.go", url: query }));
      return;
    }

    const hasScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(query);
    const looksLikeHost = !query.includes(" ") && /^[\w.-]+\.[a-z]{2,}(?:[/:?#].*)?$/i.test(query);
    if (hasScheme) {
      window.location.href = query;
    } else if (looksLikeHost) {
      window.location.href = "https://" + query;
    } else {
      window.location.href = "https://duckduckgo.com/?q=" + encodeURIComponent(query);
    }
  });

  input.focus();
})();