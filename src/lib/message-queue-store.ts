import type { QueuedMessage } from "@/hooks/use-message-queue"
import { randomUUID } from "@/lib/utils"

const PREFIX = "onehall:message-queue:v1:"
export type QueueOutcome = "accepted" | "busy" | "unknown"
interface RecordRow {
  item: QueuedMessage
  order: number
}
interface Delivery {
  status: "queued" | "unknown" | "accepted"
  token: string
  item?: QueuedMessage
}

function cleanDraft(item: QueuedMessage): QueuedMessage {
  return {
    id: item.id,
    draft: item.draft,
    modeId: item.modeId,
    ...(item.adoptSendTimeMode !== undefined
      ? { adoptSendTimeMode: item.adoptSendTimeMode }
      : {}),
  }
}

// IndexedDB read-write transactions also serialize across windows, including
// HTTP origins / older webviews where the Web Locks API is unavailable. The
// transaction encloses only synchronous claim writes, never network IO.
function withQueueLock<T>(key: string, action: () => T): Promise<T> {
  if (navigator.locks) return navigator.locks.request(key, action)
  if (typeof indexedDB === "undefined")
    return Promise.reject(new Error("Queue coordination unavailable"))
  return new Promise((resolve, reject) => {
    let abandoned = false
    const open = indexedDB.open("onehall-queue-coordination-v1", 1)
    open.onupgradeneeded = () => open.result.createObjectStore("claims")
    open.onerror = () => reject(open.error)
    open.onblocked = () => {
      abandoned = true
      reject(new Error("Queue coordination blocked"))
    }
    open.onsuccess = () => {
      const db = open.result
      if (abandoned) {
        db.close()
        return
      }
      try {
        const transaction = db.transaction("claims", "readwrite")
        let result: T
        transaction.objectStore("claims").get(key).onsuccess = () => {
          try {
            result = action()
          } catch (error) {
            transaction.abort()
            reject(error)
          }
        }
        transaction.oncomplete = () => {
          db.close()
          resolve(result)
        }
        transaction.onabort = transaction.onerror = () => {
          db.close()
          reject(transaction.error)
        }
      } catch (error) {
        db.close()
        reject(error)
      }
    }
  })
}

// Each draft and its delivery marker have separate keys. A stale reorder/edit
// cannot overwrite a concurrent claim or resurrect an accepted request. Keep
// small id-only acceptance tombstones; never keep accepted message bodies.
export function createMessageQueueStore(scope?: string) {
  const prefix = scope ? `${PREFIX}${encodeURIComponent(scope)}:` : null
  let rows: QueuedMessage[] = []
  let records = new Map<string, RecordRow>()
  const deliveries = new Map<string, Delivery>()
  const active = new Map<string, string>()
  const listeners = new Set<() => void>()
  let persistenceError = false
  const emit = () => listeners.forEach((listener) => listener())
  const fail = () => {
    persistenceError = true
    emit()
  }
  const deliveryKey = (id: string) => prefix + "delivery:" + id
  function delivery(id: string): Delivery | undefined {
    if (!prefix || persistenceError) return deliveries.get(id)
    const raw = localStorage.getItem(deliveryKey(id))
    if (!raw) return undefined
    const value = JSON.parse(raw) as Delivery
    if (
      !["queued", "unknown", "accepted"].includes(value.status) ||
      typeof value.token !== "string"
    )
      throw new Error("Invalid delivery marker")
    deliveries.set(id, value)
    return value
  }
  function writeDelivery(id: string, value: Delivery): boolean {
    try {
      if (prefix) localStorage.setItem(deliveryKey(id), JSON.stringify(value))
    } catch {
      fail()
      return false
    }
    deliveries.set(id, value)
    return true
  }
  function refresh() {
    if (prefix && !persistenceError) {
      try {
        const next = new Map<string, RecordRow>()
        for (let i = 0; i < localStorage.length; i++) {
          const key = localStorage.key(i)
          if (!key?.startsWith(prefix + "draft:")) continue
          const value = JSON.parse(localStorage.getItem(key)!) as RecordRow
          const item = value?.item
          if (
            !item ||
            typeof item.id !== "string" ||
            key !== prefix + "draft:" + item.id ||
            !Array.isArray(item.draft?.blocks) ||
            typeof item.draft.displayText !== "string" ||
            !(item.modeId === null || typeof item.modeId === "string") ||
            !Number.isFinite(value.order)
          )
            throw new Error("Invalid queue record")
          next.set(item.id, value)
        }
        records = next
      } catch {
        persistenceError = true
      }
    }
    const loaded: RecordRow[] = []
    for (const record of records.values()) {
      let state: Delivery | undefined
      try {
        state = delivery(record.item.id)
      } catch {
        persistenceError = true
      }
      if (state?.status === "accepted") continue
      const item = state?.item ?? record.item
      loaded.push({
        order: record.order,
        item: {
          ...item,
          ...(state?.status === "unknown"
            ? {
                status:
                  active.get(item.id) === state.token ? "sending" : "unknown",
              }
            : state?.status === "queued"
              ? { status: "queued" }
              : {}),
        },
      })
    }
    const nextRows = loaded
      .sort((a, b) => a.order - b.order || a.item.id.localeCompare(b.item.id))
      .map((row) => row.item)
    if (JSON.stringify(nextRows) !== JSON.stringify(rows)) rows = nextRows
  }
  refresh()
  function commit(next: QueuedMessage[]): boolean {
    const previous = records
    const updated = new Map<string, RecordRow>()
    let ok = true
    for (const [order, item] of next.entries()) {
      const draft = cleanDraft(item)
      const old = previous.get(item.id)
      let state: Delivery | undefined
      try {
        state = delivery(item.id)
      } catch {
        fail()
        ok = false
      }
      if (state?.status === "accepted") continue
      // Pending/unknown payload is immutable; a concurrent stale editor may
      // only observe the retained snapshot, never change what was sent.
      const record =
        state?.status === "unknown" && old ? old : { item: draft, order }
      updated.set(item.id, record)
      if (prefix && JSON.stringify(old) !== JSON.stringify(record)) {
        try {
          localStorage.setItem(
            prefix + "draft:" + item.id,
            JSON.stringify(record)
          )
        } catch {
          persistenceError = true
          ok = false
        }
      }
    }
    for (const id of previous.keys())
      if (!updated.has(id)) {
        if (!writeDelivery(id, { status: "accepted", token: "removed" }))
          ok = false
        try {
          if (prefix) localStorage.removeItem(prefix + "draft:" + id)
        } catch {
          persistenceError = true
          ok = false
        }
      }
    records = updated
    refresh()
    emit()
    return ok
  }
  const store = {
    getSnapshot: () => rows,
    getError: () => persistenceError,
    failPersistence: fail,
    read: () => {
      refresh()
      return rows
    },
    commit,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      const onStorage = (event: StorageEvent) => {
        if (event.key === null || event.key.startsWith(prefix ?? "\0")) {
          refresh()
          emit()
        }
      }
      if (prefix) window.addEventListener("storage", onStorage)
      return () => {
        listeners.delete(listener)
        if (prefix) window.removeEventListener("storage", onStorage)
      }
    },
    async beginSend(
      id?: string,
      sendTimeMode?: string | null
    ): Promise<QueuedMessage | undefined> {
      const claim = () => {
        const current = store.read()
        if (!id && current.some((row) => row.status && row.status !== "queued"))
          return
        let item = id ? current.find((row) => row.id === id) : current[0]
        if (
          !item ||
          (item.status && item.status !== "queued") ||
          persistenceError
        )
          return
        if (item.adoptSendTimeMode && sendTimeMode !== undefined)
          item = { ...item, modeId: sendTimeMode, adoptSendTimeMode: false }
        const token = randomUUID()
        const claimed = { ...item, deliveryToken: token }
        if (
          !writeDelivery(item.id, { status: "unknown", token, item: claimed })
        )
          return
        active.set(item.id, token)
        refresh()
        emit()
        return claimed
      }
      if (!prefix) return claim()
      try {
        return await withQueueLock(prefix, claim)
      } catch {
        fail()
        return
      }
    },
    settle(id: string, outcome: QueueOutcome, token?: string) {
      const owned = active.get(id)
      if (!owned || (token && token !== owned)) return
      active.delete(id)
      let state: Delivery | undefined
      try {
        state = delivery(id)
      } catch {
        fail()
        return
      }
      if (state?.token !== owned) return
      if (outcome === "accepted") {
        if (writeDelivery(id, { status: "accepted", token: owned })) {
          records.delete(id)
          try {
            if (prefix) localStorage.removeItem(prefix + "draft:" + id)
          } catch {
            fail()
          }
        }
      } else if (outcome === "busy") {
        // The backend explicitly rejected before accepting. Preserve the exact
        // first-attempt mode and payload, including adopted send-time mode.
        const item = state.item!
        const record = {
          item: cleanDraft(item),
          order: records.get(id)?.order ?? 0,
        }
        try {
          if (prefix)
            localStorage.setItem(prefix + "draft:" + id, JSON.stringify(record))
          records.set(id, record)
        } catch {
          fail()
        }
        if (!persistenceError)
          writeDelivery(id, { status: "queued", token: owned })
      }
      refresh()
      emit()
    },
    retryUnknown(id: string) {
      const restore = () => {
        const current = store.read().find((item) => item.id === id)
        if (
          !current ||
          current.status !== "unknown" ||
          active.has(id) ||
          persistenceError
        )
          return
        const item = cleanDraft(current)
        const record = { item, order: records.get(id)?.order ?? 0 }
        try {
          if (prefix)
            localStorage.setItem(prefix + "draft:" + id, JSON.stringify(record))
          records.set(id, record)
        } catch {
          fail()
          return
        }
        writeDelivery(id, { status: "queued", token: randomUUID() })
        refresh()
        emit()
      }
      if (!prefix) return restore()
      void withQueueLock(prefix, restore).catch(fail)
    },
  }
  return store
}

const stores = new Map<string, ReturnType<typeof createMessageQueueStore>>()
export function getMessageQueueStore(scope: string) {
  let store = stores.get(scope)
  if (!store) {
    store = createMessageQueueStore(scope)
    stores.set(scope, store)
  }
  return store
}

/** Draft tab ids become canonical conversation ids on restart. The alias
 * keeps a pending request and its original settlement in the same store. */
export function resolveMessageQueueScope(
  backend: string,
  tabId: string,
  conversationId: number | null,
  previous?: string
): string {
  if (previous) {
    try {
      const identity = JSON.parse(previous)
      if (
        identity[0] !== backend ||
        identity[1] !== "draft" ||
        identity[2] !== tabId
      )
        previous = undefined
    } catch {
      previous = undefined
    }
  }
  if (conversationId === null)
    return previous ?? JSON.stringify([backend, "draft", tabId])
  const canonical = JSON.stringify([backend, "conversation", conversationId])
  const key = PREFIX + "alias:" + encodeURIComponent(canonical)
  const scope = previous ?? canonical
  if (typeof window === "undefined") return scope
  try {
    const existing = localStorage.getItem(key)
    if (existing) {
      const identity = JSON.parse(existing)
      if (!Array.isArray(identity) || identity[0] !== backend)
        throw new Error("Invalid queue scope")
      return existing
    }
    localStorage.setItem(key, scope)
  } catch {
    getMessageQueueStore(scope).failPersistence()
  }
  return scope
}
