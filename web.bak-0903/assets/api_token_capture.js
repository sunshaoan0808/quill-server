/**
 * Auth token 捕获器 — 独立 JS 模块（st_presets.js / realm_market.js 共用）
 *
 * app.js 的认证 token 存于闭包变量（不可直接读取），但每次 fetch 请求都会带
 * `Authorization: Bearer <token>` header。本模块 patch window.fetch，
 * 从任意带 Bearer 的请求中捕获 token 并缓存到 localStorage('museai_token')，
 * 供后续独立 JS 的 API 调用复用。幂等：多次加载只 patch 一次。
 */
(function () {
  'use strict';

  if (window.__museaiTokenCaptureInstalled) return;
  window.__museaiTokenCaptureInstalled = true;

  function readTokenFromHeaders(headers) {
    try {
      if (!headers) return null;
      // Headers 对象（fetch init）可能为普通对象或 Headers 实例
      if (typeof headers.get === 'function') {
        var h = headers.get('Authorization');
        if (h && h.indexOf('Bearer ') === 0) return h.slice(7).trim();
        return null;
      }
      if (headers.Authorization && headers.Authorization.indexOf('Bearer ') === 0) {
        return headers.Authorization.slice(7).trim();
      }
      if (headers.authorization && headers.authorization.indexOf('Bearer ') === 0) {
        return headers.authorization.slice(7).trim();
      }
    } catch (e) {}
    return null;
  }

  function readTokenFromResponse(res) {
    try {
      if (res && res.headers && typeof res.headers.get === 'function') {
        var h = res.headers.get('x-auth-token') || res.headers.get('X-Auth-Token');
        if (h) return h.trim();
      }
    } catch (e) {}
    return null;
  }

  var origFetch = window.fetch;
  window.fetch = function (input, init) {
    var promise = origFetch.apply(this, arguments);

    // 从请求头捕获 token
    var reqToken = null;
    try {
      if (init) reqToken = readTokenFromHeaders(init.headers);
    } catch (e) {}
    if (reqToken) {
      try { localStorage.setItem('museai_token', reqToken); } catch (e) {}
    }

    // 从响应头捕获（fallback：部分端点把 token 放响应头）
    if (promise && typeof promise.then === 'function') {
      promise.then(function (res) {
        var respToken = readTokenFromResponse(res);
        if (respToken) {
          try { localStorage.setItem('museai_token', respToken); } catch (e) {}
        }
        return res;
      }).catch(function () {});
    }

    return promise;
  };
})();
