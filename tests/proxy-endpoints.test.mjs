import assert from "node:assert/strict";
import { test } from "node:test";
import { proxyEndpoints } from "../src/api.ts";

const state = (server, endpoints = {}) => ({
  enable: true, server, bypass: "", pac_present: false, ...endpoints,
});

test("legacy SOCKS state never becomes an HTTP hostname containing socks=", () => {
  assert.deepEqual(proxyEndpoints(state("socks=127.0.0.1:1080")), [
    { label: "SOCKS", addr: "127.0.0.1:1080" },
  ]);
});

test("legacy segmented state keeps each protocol separate", () => {
  assert.deepEqual(proxyEndpoints(state(" HTTP=localhost:7890; HTTPS=localhost:7891; socks5=localhost:1080;ftp=localhost:21")), [
    { label: "HTTP", addr: "localhost:7890" },
    { label: "HTTPS", addr: "localhost:7891" },
    { label: "SOCKS", addr: "localhost:1080" },
  ]);
});

test("HTTP state after SOCKS switch contains no stale SOCKS endpoint", () => {
  assert.deepEqual(proxyEndpoints(state("localhost:7890", { http_proxy: "localhost:7890", socks_proxy: null })), [
    { label: "HTTP", addr: "localhost:7890" },
  ]);
});

test("bare legacy HTTP and bracketed IPv6 text are classified as HTTP", () => {
  assert.deepEqual(proxyEndpoints(state("[::1]:7890")), [{ label: "HTTP", addr: "[::1]:7890" }]);
  assert.deepEqual(proxyEndpoints(state("")), []);
  assert.deepEqual(proxyEndpoints(state("socks=;ftp=localhost:21")), []);
});
