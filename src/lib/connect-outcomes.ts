// Realm-scoped submissions must survive workspace/provider remounts. A lost
// HTTP result does not prove backend cancellation. Entries contain only local
// request identities and are retired on a definitive response or discovery.
export const uncertainConnectRequests = new Set<string>()
