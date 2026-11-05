import { detectEnvironment } from "./detect"
import type { EventStream, RemoteTransportConfig, Transport } from "./types"

export type { RemoteTransportConfig, Transport, UnsubscribeFn } from "./types"

let _shellTransport: Transport | null = null
let _remoteTransport: Transport | null = null
let _remoteConfig: RemoteTransportConfig | null = null
let _webBranchTransport: Transport | null = null
let _webBranchId: string | null = null

// onehall: an opened workspace tab keeps its connection until the tab is
// closed or the program exits. These registries own the transports; the
// `_remoteTransport` / `_webBranchTransport` pointers above only say which one
// `getTransport()` routes to right now, so a tab switch re-routes and never
// destroys or re-dials.
interface RemoteEntry {
  transport: Transport
  /** The config the transport was built with (its 401 hook is the entry's). */
  config: RemoteTransportConfig
  /** The mounted gate's 401 handler; none while the tab is in the background. */
  handler: (() => void) | undefined
  /** A 401 arrived while no gate was listening; replayed to the next one. */
  unauthorizedPending: boolean
}
const _remoteEntries = new Map<number, RemoteEntry>()
const _webBranchEntries = new Map<string, Transport>()

// Test-only overrides for the lazily `require`d transports (vitest cannot
// resolve a CommonJS `require` of a TS module). Null in production.
let _testFactories: {
  tauri?: () => Transport
  remote?: (config: RemoteTransportConfig) => Transport
  web?: (baseUrl: string) => Transport
} | null = null

function createTauriTransport(): Transport {
  if (_testFactories?.tauri) return _testFactories.tauri()
  // Use dynamic require to avoid bundling tauri deps in web mode.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { TauriTransport } = require("./tauri-transport") as {
    TauriTransport: new () => Transport
  }
  return new TauriTransport()
}

function createWebTransport(baseUrl: string): Transport {
  if (_testFactories?.web) return _testFactories.web(baseUrl)
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { WebTransport } = require("./web-transport") as {
    WebTransport: new (baseUrl: string) => Transport
  }
  return new WebTransport(baseUrl)
}

/** The browser keeps headquarters authentication; only the approved branch
 * path changes. This is never a user-provided remote URL. A branch already
 * opened in this page keeps its transport. */
export function configureWebBranchTransport(branchId: string): void {
  if (
    isDesktop() ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
      branchId
    )
  )
    throw new Error("Invalid browser branch selector")
  let transport = _webBranchEntries.get(branchId)
  if (!transport) {
    transport = createWebTransport(
      `${window.location.origin}/branches/${branchId}`
    )
    _webBranchEntries.set(branchId, transport)
  }
  _webBranchId = branchId
  _webBranchTransport = transport
}

/** Tab closed: drop that branch's browser connection. */
export function releaseWebBranchTransport(branchId: string): void {
  _webBranchEntries.get(branchId)?.destroy?.()
  _webBranchEntries.delete(branchId)
  if (_webBranchId === branchId) {
    _webBranchTransport = null
    _webBranchId = null
  }
}

export function getActiveBackendKey(): string {
  if (_webBranchId) return `branch:${_webBranchId}`
  return _remoteConfig ? `remote:${_remoteConfig.id}` : "local"
}

export function isBranchWorkspace(): boolean {
  return (
    _webBranchId !== null ||
    (_remoteConfig !== null &&
      /\/branches\/[0-9a-f-]{36}\/?$/.test(_remoteConfig.baseUrl))
  )
}

export function getShellTransport(): Transport {
  if (!_shellTransport) {
    const env = detectEnvironment()
    _shellTransport =
      env === "tauri"
        ? createTauriTransport()
        : createWebTransport(window.location.origin)
  }
  return _shellTransport
}

function sameRemoteTarget(a: RemoteTransportConfig, b: RemoteTransportConfig) {
  return (
    a.baseUrl.replace(/\/+$/, "") === b.baseUrl.replace(/\/+$/, "") &&
    a.token === b.token &&
    a.windowInstanceId === b.windowInstanceId
  )
}

function destroyRemoteEntry(id: number): void {
  _remoteEntries.get(id)?.transport.destroy?.()
  _remoteEntries.delete(id)
}

/** Route to the remote for `config`. A transport already open for the same
 * address, token and window keeps its connection; changed credentials replace
 * it. */
export function configureRemoteDesktopTransport(
  config: RemoteTransportConfig
): void {
  let entry = _remoteEntries.get(config.id)
  if (entry && !sameRemoteTarget(entry.config, config)) {
    destroyRemoteEntry(config.id)
    entry = undefined
  }
  if (!entry) {
    const created = { handler: undefined, unauthorizedPending: false } as Pick<
      RemoteEntry,
      "handler" | "unauthorizedPending"
    >
    const wired: RemoteTransportConfig = {
      ...config,
      onUnauthorized: () => {
        if (created.handler) created.handler()
        else created.unauthorizedPending = true
      },
    }
    let transport: Transport
    if (_testFactories?.remote) {
      transport = _testFactories.remote(wired)
    } else {
      const { RemoteDesktopTransport } =
        // eslint-disable-next-line @typescript-eslint/no-require-imports
        require("./remote-desktop-transport") as {
          RemoteDesktopTransport: new (
            config: RemoteTransportConfig
          ) => Transport
        }
      transport = new RemoteDesktopTransport(wired)
    }
    entry = Object.assign(created, { transport, config: wired }) as RemoteEntry
    _remoteEntries.set(config.id, entry)
  }
  entry.handler = config.onUnauthorized
  _remoteConfig = entry.config
  _remoteTransport = entry.transport
  if (entry.unauthorizedPending) {
    entry.unauthorizedPending = false
    entry.handler?.()
  }
}

/** Stop routing to the current remote but keep its connection open. */
export function detachRemoteDesktopTransport(): void {
  for (const entry of _remoteEntries.values()) entry.handler = undefined
  _remoteTransport = null
  _remoteConfig = null
}

/** Destroy the currently routed remote (a gate that could not load it). */
export function clearRemoteDesktopTransport(): void {
  if (_remoteConfig) destroyRemoteEntry(_remoteConfig.id)
  detachRemoteDesktopTransport()
}

/** Tab closed: drop that remote's connection and subscriptions. */
export function releaseRemoteDesktopTransport(id: number): void {
  destroyRemoteEntry(id)
  if (_remoteConfig?.id === id) {
    _remoteTransport = null
    _remoteConfig = null
  }
}

/** Program or shell teardown: release every workspace connection held here. */
export function releaseAllWorkspaceTransports(): void {
  for (const id of [..._remoteEntries.keys()]) destroyRemoteEntry(id)
  for (const t of _webBranchEntries.values()) t.destroy?.()
  _webBranchEntries.clear()
  _remoteTransport = null
  _remoteConfig = null
  _webBranchTransport = null
  _webBranchId = null
}

export function getActiveRemoteConnectionId(): number | null {
  return _remoteConfig?.id ?? null
}

// onehall: single-window mode swaps the backend behind `getTransport()` in
// place (bottom workspace tabs). Every switch bumps this epoch; a call or
// event that belongs to an earlier epoch is dropped instead of being handed
// to the new workspace, so a slow answer from the backend we just left can
// never land in the stores of the one we switched to.
let _backendEpoch = 0
let _guarded = new WeakMap<Transport, Transport>()

/** A promise that never settles: the caller belongs to a workspace that is
 *  gone, so neither its success nor its error path may run. */
function abandoned<T>(): Promise<T> {
  return new Promise<T>(() => {})
}

function guardTransport(target: Transport): Transport {
  const cached = _guarded.get(target)
  if (cached) return cached
  const epoch = _backendEpoch
  const proxy = new Proxy(target, {
    get(obj, prop, receiver) {
      if (prop === "call") {
        return <T>(
          command: string,
          args?: Record<string, unknown>,
          options?: Parameters<Transport["call"]>[2]
        ): Promise<T> => {
          return obj.call<T>(command, args, options).then(
            (value) => (epoch === _backendEpoch ? value : abandoned<T>()),
            (err) =>
              epoch === _backendEpoch ? Promise.reject(err) : abandoned<T>()
          )
        }
      }
      if (prop === "subscribe") {
        return <T>(event: string, handler: (payload: T) => void) => {
          return obj.subscribe<T>(event, (payload) => {
            if (epoch === _backendEpoch) handler(payload)
          })
        }
      }
      if ((prop === "onReady" || prop === "onReconnect") && obj[prop]) {
        return (handler: () => void) => {
          return obj[prop]?.(() => {
            if (epoch === _backendEpoch) handler()
          })
        }
      }
      if (prop === "eventStream" && obj.eventStream) {
        return (): EventStream => {
          const stream = obj.eventStream!()
          return {
            attach: (id, options, handlers) =>
              stream.attach(id, options, {
                onSnapshot: (...args) => {
                  if (epoch === _backendEpoch) handlers.onSnapshot(...args)
                },
                onReplay: (...args) => {
                  if (epoch === _backendEpoch) handlers.onReplay(...args)
                },
                onEvent: (...args) => {
                  if (epoch === _backendEpoch) handlers.onEvent(...args)
                },
                onDetached: (...args) => {
                  if (epoch === _backendEpoch) handlers.onDetached(...args)
                },
              }),
          }
        }
      }
      const value = Reflect.get(obj, prop, receiver)
      return typeof value === "function" ? value.bind(obj) : value
    },
  })
  _guarded.set(target, proxy)
  return proxy
}

/**
 * onehall: called by the single-window shell between unmounting one
 * workspace and mounting the next — after the old realm is gone, before any
 * component of the new one renders. Invalidates everything still in flight
 * and drops the remote transport so the next realm starts on the shell; a
 * remote realm reconfigures it in `RemoteConnectionGate` before its
 * children mount.
 */
export function beginBackendSwitch(): void {
  _backendEpoch += 1
  _guarded = new WeakMap()
  // Re-route only: opened tabs keep their connections in the registries.
  detachRemoteDesktopTransport()
  _webBranchTransport = null
  _webBranchId = null
}

export function getTransport(): Transport {
  return guardTransport(
    _webBranchTransport ?? _remoteTransport ?? getShellTransport()
  )
}

/** OneHall: an explicitly submitted write must finish on its original backend.
 * UI reads/events still use getTransport() and its epoch guard. Never use this
 * handle to publish a late result into backend-scoped stores. */
export function captureSubmissionBackend() {
  const transport =
    _webBranchTransport ?? _remoteTransport ?? getShellTransport()
  const epoch = _backendEpoch
  const remoteId = _remoteConfig?.id ?? null
  return {
    call: transport.call.bind(transport) as Transport["call"],
    key: getActiveBackendKey(),
    stripImages: !transport.isDesktop() || remoteId !== null,
    isCurrent: () => epoch === _backendEpoch,
  }
}

export function isDesktop(): boolean {
  return detectEnvironment() === "tauri"
}

/// True when the current window is a Tauri client bound to a remote
/// codeg-server (a remote-desktop window). Distinct from `isDesktop()`,
/// which is purely a runtime check — a remote-desktop window IS a Tauri
/// runtime but its API calls and file ops must target the remote host,
/// not the local filesystem.
export function isRemoteDesktopMode(): boolean {
  return _remoteTransport !== null
}

/// Base URL of the codeg server backing the current transport, for building
/// raw resource URLs that bypass the JSON transport (iframes, downloads).
/// Remote-desktop → the remote host; web → this page's origin. In pure-desktop
/// mode there's no server (callers use loopback / `invoke`), so this returns
/// the local origin only as a harmless fallback.
export function getServerBaseUrl(): string {
  if (_webBranchId) return `${window.location.origin}/branches/${_webBranchId}`
  if (_remoteConfig) return _remoteConfig.baseUrl.replace(/\/+$/, "")
  return typeof window !== "undefined" ? window.location.origin : ""
}

/// Surface a remote-server 401 to the same UI the transport uses for its
/// own auth failures. Direct `invoke()` calls (workspace file
/// upload/download) bypass `RemoteDesktopTransport.call`, so without
/// this they'd toast "token invalid" but never raise the
/// `connection-expired` dialog the rest of the app uses. Calling this on
/// a non-remote-desktop window is a no-op so the helper is safe to use
/// unconditionally from a 401 catch block.
export function notifyRemoteDesktopUnauthorized(): void {
  _remoteConfig?.onUnauthorized?.()
}

/**
 * Test-only: clear the cached shell + remote transports so a subsequent
 * `getTransport()` / `getShellTransport()` call re-runs environment detection
 * against the current `window` mock. The module-level singletons would
 * otherwise stick across test cases. Not intended for production use.
 * @internal
 */
export function __resetTransportForTests(): void {
  // Hard guard: collapses to a no-op outside vitest. Turbopack/webpack DCE
  // the dead branch in `next build` so the function ships as a single
  // `return` in the prod bundle instead of reaching into module state.
  if (process.env.NODE_ENV !== "test") return
  _shellTransport?.destroy?.()
  releaseAllWorkspaceTransports()
  _shellTransport = null
  _backendEpoch = 0
  _guarded = new WeakMap()
}

/**
 * Test-only: build the shell / remote transports with these factories
 * instead of the lazily required modules. No-op outside vitest.
 * @internal
 */
export function __setTransportFactoriesForTests(factories: {
  tauri?: () => Transport
  remote?: (config: RemoteTransportConfig) => Transport
  web?: (baseUrl: string) => Transport
}): void {
  if (process.env.NODE_ENV !== "test") return
  _testFactories = factories
}
