// Узкий фасад для веб-фронта MateHub (init-script, инжектится на каждую
// страницу webview — и на локальный пикер, и на remote-хаб).
//
// Контракт зеркалит frontend/lib/native-bridge.ts (version 1). Это НЕ
// граница безопасности — ей остаётся Tauri ACL (runtime capability выдаёт
// remote-origin'у только команды шаринга + core:event) — а API-контракт,
// чтобы веб-код не знал про Tauri internals.
(function () {
  "use strict";
  if (window.__MATEHUB_NATIVE__) return;
  var internals = window.__TAURI_INTERNALS__;
  if (!internals) return;

  var invoke = function (cmd, args) {
    return internals.invoke(cmd, args || {});
  };

  // Подписка на события Tauri с fan-out нескольким слушателям.
  function subscribe(eventName) {
    var listeners = new Set();
    var started = false;
    function ensure() {
      if (started) return;
      started = true;
      invoke("plugin:event|listen", {
        event: eventName,
        target: { kind: "Any" },
        handler: internals.transformCallback(function (event) {
          listeners.forEach(function (cb) {
            try {
              cb(event.payload);
            } catch (e) {
              console.error(eventName + " listener failed", e);
            }
          });
        }),
      }).catch(function (e) {
        started = false;
        console.error("failed to subscribe to " + eventName, e);
      });
    }
    return function (cb) {
      listeners.add(cb);
      ensure();
      return function () {
        listeners.delete(cb);
      };
    };
  }

  var onShareStatus = subscribe("share-status");
  var onVoiceStatus = subscribe("voice-status");

  window.__MATEHUB_NATIVE__ = Object.freeze({
    version: 1,
    // Screen share (нативный захват+кодек).
    listShareTargets: function () {
      return invoke("list_targets");
    },
    startShare: function (config) {
      return invoke("start_share", { config: config });
    },
    stopShare: function () {
      return invoke("stop_share");
    },
    onShareStatus: onShareStatus,
    // Нативный войс (микрофон + приём). На десктопе webview в WebRTC-войс
    // не входит — им владеет этот клиент.
    joinVoice: function (config) {
      return invoke("join_voice", { config: config });
    },
    leaveVoice: function () {
      return invoke("leave_voice");
    },
    setMute: function (muted) {
      return invoke("set_mute", { muted: muted });
    },
    setDeafen: function (deafened) {
      return invoke("set_deafen", { deafened: deafened });
    },
    onVoiceStatus: onVoiceStatus,
  });
})();
