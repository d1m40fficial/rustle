(function () {
  "use strict";
  var image = document.getElementById("surface");
  var status = document.getElementById("surface-status");

  function send(message) {
    if (window.ipc) window.ipc.postMessage(JSON.stringify(message));
  }

  window.rustleObscuraSurface = {
    update: function (state) {
      if (state.screenshot) {
        image.src = state.screenshot;
        status.hidden = true;
      }
      if (state.error) {
        status.hidden = false;
        status.textContent = state.error;
      } else if (state.status) {
        status.textContent = state.status;
      }
    }
  };

  image.addEventListener("click", function (event) {
    var rect = image.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    var x = (event.clientX - rect.left) / rect.width * (image.naturalWidth || rect.width);
    var y = (event.clientY - rect.top) / rect.height * (image.naturalHeight || rect.height);
    send({ type: "obscura.surface.click", x: x, y: y });
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Enter" || event.key === "Backspace" || event.key.length === 1) {
      send({ type: "obscura.surface.key", key: event.key });
    }
  });
  send({ type: "obscura.surface.ready" });
})();
