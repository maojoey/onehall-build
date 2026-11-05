import { createHash, randomUUID } from "node:crypto"
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { basename, dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

export const DESKTOP_PLATFORMS = Object.freeze({
  "mac-x64": {
    target: "x86_64-apple-darwin",
    bundle: "dmg",
    updaterPlatform: "darwin-x86_64",
    installerSuffix: "_x64.dmg",
  },
  "mac-arm64": {
    target: "aarch64-apple-darwin",
    bundle: "dmg",
    updaterPlatform: "darwin-aarch64",
    installerSuffix: "_aarch64.dmg",
  },
  "win-x64": {
    target: "x86_64-pc-windows-msvc",
    bundle: "nsis",
    updaterPlatform: "windows-x86_64",
    installerSuffix: "_x64-setup.exe",
  },
})

// The inherited Codeg key must never authorize a OneHall release.
const CODEG_PUBLIC_KEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQ4OEM3NkMyMTVENjdBODgKUldTSWV0WVZ3bmFNU0NnSzhpdFg2bXFmMHFidWd1eWpuZ2Y2WmU5QmVXdWVrU0ZpOCt2dnd6WW4K"

export function validateProvenance({ version, commit, run }) {
  if (
    typeof version !== "string" ||
    !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(
      version
    )
  ) {
    throw new Error("Expected an explicit semantic release version")
  }
  if (typeof commit !== "string" || !/^[a-f0-9]{40}$/.test(commit)) {
    throw new Error("Expected a full lowercase Git commit SHA")
  }
  if (
    typeof run !== "string" ||
    !/^(?:[1-9]\d*|local-[A-Za-z0-9][A-Za-z0-9_-]{0,79})$/.test(run)
  ) {
    throw new Error(
      "Expected a numeric Actions run ID or safe local-<id> provenance"
    )
  }
}

export function validatePlatform(platform, target, bundles) {
  const spec = DESKTOP_PLATFORMS[platform]
  if (!spec || spec.target !== target || spec.bundle !== bundles) {
    throw new Error("Conflicting or unsupported platform, target, or bundle")
  }
  return spec
}

export function validateBasename(name) {
  if (
    typeof name !== "string" ||
    !name ||
    name === "." ||
    name === ".." ||
    /[/\\\x00-\x1f\x7f]/.test(name) ||
    basename(name) !== name
  ) {
    throw new Error("Invalid artifact basename")
  }
  return name
}

function decodeBase64(value, label) {
  if (
    typeof value !== "string" ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      value
    ) ||
    !value
  ) {
    throw new Error(`Invalid ${label} base64`)
  }
  const bytes = Buffer.from(value, "base64")
  if (bytes.toString("base64") !== value) {
    throw new Error(`Invalid ${label} base64 encoding`)
  }
  return bytes
}

// This checks the Tauri/minisign envelope only. It does NOT prove authenticity.
export function validateSignatureStructure(text) {
  const signature = text.trim()
  const decoded = decodeBase64(signature, "signature").toString("utf8")
  const lines = decoded.trimEnd().split(/\r?\n/)
  if (
    lines.length !== 4 ||
    !lines[0].startsWith("untrusted comment: ") ||
    !lines[2].startsWith("trusted comment: ") ||
    lines[2].length <= "trusted comment: ".length
  ) {
    throw new Error("Invalid minisign signature envelope")
  }
  const packet = decodeBase64(lines[1], "signature packet")
  const globalSignature = decodeBase64(lines[3], "global signature")
  if (
    packet.length !== 74 ||
    !["Ed", "ED"].includes(packet.subarray(0, 2).toString("ascii")) ||
    globalSignature.length !== 64
  ) {
    throw new Error("Invalid minisign signature packet")
  }
  return signature
}

export function validateHTTPSURL(value) {
  // Check raw path before URL normalization can erase traversal segments.
  if (
    typeof value !== "string" ||
    !/^https:\/\//i.test(value) ||
    /[\s\\\x00-\x1f\x7f]/.test(value)
  ) {
    throw new Error("Expected an HTTPS URL without whitespace or backslashes")
  }
  let url
  try {
    url = new URL(value)
    const rawPath = value.replace(/^https:\/\/[^/]+/i, "").split(/[?#]/)[0]
    for (const segment of rawPath.split("/")) {
      const decoded = decodeURIComponent(segment)
      if (decoded === "." || decoded === ".." || /[/\\]/.test(decoded)) {
        throw new Error("Unsafe URL path")
      }
    }
  } catch {
    throw new Error("Invalid HTTPS URL or unsafe URL path")
  }
  if (
    url.protocol !== "https:" ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error(
      "Expected HTTPS URL without credentials, query, or fragment"
    )
  }
  return url
}

export function validateSigningConfiguration({
  policy,
  baseConfig,
  hallConfig,
  hasPrivateKey,
}) {
  if (!hasPrivateKey) {
    throw new Error("Signed updates require ONEHALL_TAURI_SIGNING_PRIVATE_KEY")
  }
  if (policy.product !== "onehall" || policy.updateMode !== "desktop-signed") {
    throw new Error(
      "Signed updates require OneHall desktop-signed release policy"
    )
  }
  if (policy.version !== baseConfig.version) {
    throw new Error("Release policy and Tauri versions differ")
  }
  // Require an explicit OneHall override; never silently inherit Codeg's key.
  const updater = hallConfig.plugins?.updater
  const pubkey = updater?.pubkey
  if (typeof pubkey !== "string" || pubkey === CODEG_PUBLIC_KEY) {
    throw new Error("Configure an explicit OneHall updater public key")
  }
  const lines = decodeBase64(pubkey, "public key")
    .toString("utf8")
    .trimEnd()
    .split(/\r?\n/)
  if (lines.length !== 2 || !lines[0].startsWith("untrusted comment: ")) {
    throw new Error("Invalid minisign public key envelope")
  }
  const packet = decodeBase64(lines[1], "public key packet")
  if (
    packet.length !== 42 ||
    packet.subarray(0, 2).toString("ascii") !== "Ed"
  ) {
    throw new Error("Invalid minisign public key packet")
  }
  const inheritedPacket = Buffer.from(
    Buffer.from(CODEG_PUBLIC_KEY, "base64").toString("utf8").split("\n")[1],
    "base64"
  )
  // The key ID and comment are labels, not the Ed25519 verification key.
  if (packet.subarray(10).equals(inheritedPacket.subarray(10))) {
    throw new Error("OneHall must not reuse the upstream updater key")
  }
  if (!Array.isArray(updater.endpoints) || !updater.endpoints.length) {
    throw new Error("Configure OneHall HTTPS updater endpoints")
  }
  const mergedUpdater = { ...baseConfig.plugins?.updater, ...updater }
  if (
    mergedUpdater.requireSignedVersion !== true ||
    mergedUpdater.allowDowngrades === true ||
    Object.entries(mergedUpdater).some(
      ([key, value]) => key.startsWith("dangerous") && value === true
    )
  )
    throw new Error(
      "Unsafe signed update policy: require version binding and secure transport"
    )
  const origins = updater.endpoints.map(
    (endpoint) => validateHTTPSURL(endpoint).origin
  )
  if (new Set(origins).size !== 1)
    throw new Error("Updater endpoints must share one origin")
}

// Store only public release policy and updater configuration from the exact source commit.
export function captureReleaseIdentity({ policy, baseConfig, hallConfig }) {
  const configIdentity = (config) => {
    const updater = config.plugins?.updater
    const selected =
      updater && typeof updater === "object"
        ? Object.fromEntries(
            Object.entries(updater).filter(
              ([key]) =>
                [
                  "pubkey",
                  "endpoints",
                  "requireSignedVersion",
                  "allowDowngrades",
                ].includes(key) || key.startsWith("dangerous")
            )
          )
        : updater
    return {
      ...(config.version === undefined ? {} : { version: config.version }),
      plugins: selected === undefined ? {} : { updater: selected },
    }
  }
  return {
    policy: {
      product: policy.product,
      version: policy.version,
      updateMode: policy.updateMode,
    },
    baseConfig: configIdentity(baseConfig),
    hallConfig: configIdentity(hallConfig),
  }
}

export function validateReleaseIdentity({
  identity,
  version,
  publicKey,
  publicOrigin,
}) {
  if (
    !identity?.policy ||
    !identity.baseConfig ||
    !identity.hallConfig ||
    identity.baseConfig.version !== version ||
    (identity.hallConfig.version !== undefined &&
      identity.hallConfig.version !== version)
  )
    throw new Error("Missing or mismatched committed release identity")
  validateSigningConfiguration({ ...identity, hasPrivateKey: true })
  const configuredKey = identity.hallConfig.plugins.updater.pubkey
  // Validate the supplied identity too, then compare key ID + Ed25519 key bytes.
  validateSigningConfiguration({
    ...identity,
    hallConfig: {
      ...identity.hallConfig,
      plugins: {
        updater: { ...identity.hallConfig.plugins.updater, pubkey: publicKey },
      },
    },
    hasPrivateKey: true,
  })
  const packet = (key) =>
    decodeBase64(
      decodeBase64(key, "public key")
        .toString("utf8")
        .trimEnd()
        .split(/\r?\n/)[1],
      "public key packet"
    )
  if (!packet(configuredKey).equals(packet(publicKey)))
    throw new Error(
      "Committed updater key does not match signing or publication key"
    )
  if (publicOrigin !== undefined) {
    const origin = validateHTTPSURL(publicOrigin)
    if (origin.pathname !== "/")
      throw new Error("Publication origin must not contain a path")
    const expected = new URL("/latest.json", origin).href
    if (
      identity.hallConfig.plugins.updater.endpoints.some(
        (endpoint) => validateHTTPSURL(endpoint).href !== expected
      )
    )
      throw new Error(
        "Committed updater endpoint does not match publication gateway"
      )
  }
  return true
}

export function readRegularFile(root, relativePath) {
  const parts = relativePath.split("/")
  if (!parts.length) throw new Error("Missing artifact path")
  parts.forEach(validateBasename)
  let current = resolve(root)
  if (
    !lstatSync(current).isDirectory() ||
    lstatSync(current).isSymbolicLink()
  ) {
    throw new Error("Artifact root must be a real directory")
  }
  for (const [index, part] of parts.entries()) {
    current = join(current, part)
    const stat = lstatSync(current)
    if (
      stat.isSymbolicLink() ||
      (index === parts.length - 1 ? !stat.isFile() : !stat.isDirectory())
    ) {
      throw new Error(
        "Artifact paths must contain regular files and directories"
      )
    }
  }
  const bytes = readFileSync(current)
  if (!bytes.length) throw new Error(`Empty artifact: ${relativePath}`)
  return bytes
}

// Pure selection keeps architecture and bundle decisions separate from copying.
export function selectDesktopArtifacts({
  platform,
  target,
  bundles,
  signedUpdates,
  prepareUpdates = false,
  installerNames,
  updaterNames = [],
}) {
  const spec = validatePlatform(platform, target, bundles)
  if (typeof prepareUpdates !== "boolean" || (signedUpdates && prepareUpdates))
    throw new Error("Prepared and signed modes are mutually exclusive")
  if (typeof signedUpdates !== "boolean") {
    throw new Error("signedUpdates must be a boolean")
  }
  const extension = spec.bundle === "dmg" ? ".dmg" : ".exe"
  const installers = installerNames.filter((name) => name.endsWith(extension))
  if (installers.length !== 1) {
    throw new Error("Expected exactly one installer")
  }
  if (!installers[0].endsWith(spec.installerSuffix)) {
    throw new Error("Installer architecture does not match the platform")
  }
  const result = {
    installers,
    updater: null,
    sources: [{ name: installers[0], path: `${spec.bundle}/${installers[0]}` }],
  }
  if (signedUpdates || prepareUpdates) {
    const payloads =
      spec.bundle === "dmg"
        ? updaterNames.filter((name) => name.endsWith(".app.tar.gz"))
        : installers
    if (payloads.length !== 1) {
      throw new Error("Expected exactly one updater payload")
    }
    const payload = payloads[0]
    const signature = `${payload}.sig`
    const names = spec.bundle === "dmg" ? updaterNames : installerNames
    if (signedUpdates && !names.includes(signature))
      throw new Error("Missing updater signature")
    const signatures = names.filter((name) =>
      name.endsWith(spec.bundle === "dmg" ? ".app.tar.gz.sig" : ".exe.sig")
    )
    if (signedUpdates && signatures.length !== 1)
      throw new Error("Ambiguous updater signatures")
    const dir = spec.bundle === "dmg" ? "macos" : "nsis"
    result.updater = { payload, signature: signedUpdates ? signature : null }
    if (!result.sources.some((source) => source.name === payload)) {
      result.sources.push({ name: payload, path: `${dir}/${payload}` })
    }
    if (signedUpdates)
      result.sources.push({ name: signature, path: `${dir}/${signature}` })
  }
  result.sources.forEach(({ name }) => validateBasename(name))
  return result
}

export function collectDesktopArtifacts({
  bundleRoot,
  outputDir,
  platform,
  target,
  bundles,
  signedUpdates = false,
  prepareUpdates = false,
  releaseIdentity,
  version,
  commit,
  run,
}) {
  validateProvenance({ version, commit, run })
  validatePlatform(platform, target, bundles)
  const list = (directory) => {
    // Validate the root and each selected directory before listing entries.
    const directoryPath = join(bundleRoot, directory)
    for (const path of [bundleRoot, directoryPath]) {
      const stat = lstatSync(path)
      if (stat.isSymbolicLink() || !stat.isDirectory()) {
        throw new Error("Bundle paths must be real directories")
      }
    }
    return readdirSync(directoryPath).filter((name) => {
      const stat = lstatSync(join(directoryPath, name))
      if (stat.isSymbolicLink()) throw new Error("Symlink in bundle directory")
      return stat.isFile()
    })
  }
  const selection = selectDesktopArtifacts({
    platform,
    target,
    bundles,
    signedUpdates,
    prepareUpdates,
    installerNames: list(bundles),
    updaterNames:
      (signedUpdates || prepareUpdates) && bundles === "dmg"
        ? list("macos")
        : [],
  })
  const contents = selection.sources.map((source) => ({
    ...source,
    bytes: readRegularFile(bundleRoot, source.path),
  }))
  if (
    !selection.installers[0].endsWith(
      `_${version}${DESKTOP_PLATFORMS[platform].installerSuffix}`
    )
  ) {
    throw new Error("Installer version does not match release provenance")
  }
  if (selection.updater?.signature) {
    validateSignatureStructure(
      contents
        .find(({ name }) => name === selection.updater.signature)
        .bytes.toString("utf8")
    )
  }
  const info = {
    schemaVersion: 1,
    version,
    platform,
    target,
    bundles,
    commit,
    run,
    signedRelease: false,
    updaterSigned: signedUpdates,
    updaterPrepared: prepareUpdates,
    osCodeSigned: false,
    signatureAuthenticityVerified: false,
    ...(releaseIdentity === undefined ? {} : { releaseIdentity }),
    installers: selection.installers,
    updater: selection.updater,
    files: contents.map(({ name, bytes }) => ({
      name,
      size: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
    })),
  }
  if (existsSync(outputDir)) throw new Error("Output directory already exists")
  mkdirSync(dirname(resolve(outputDir)), { recursive: true })
  const staging = `${resolve(outputDir)}.${randomUUID()}.tmp`
  mkdirSync(staging)
  try {
    for (const { name, bytes } of contents) {
      writeFileSync(join(staging, name), bytes)
    }
    writeFileSync(
      join(staging, "build-info.json"),
      JSON.stringify(info, null, 2) + "\n"
    )
    writeFileSync(
      join(staging, "SHA256SUMS"),
      info.files.map(({ name, sha256 }) => `${sha256}  ${name}\n`).join("")
    )
    renameSync(staging, outputDir)
  } finally {
    rmSync(staging, { recursive: true, force: true })
  }
  return info
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    if (process.argv.slice(2).join(" ") === "--check-signing-config") {
      const read = (path) => JSON.parse(readFileSync(path, "utf8"))
      validateSigningConfiguration({
        policy: read("onehall.release.json"),
        baseConfig: read("src-tauri/tauri.conf.json"),
        hallConfig: read("src-tauri/tauri.hall.conf.json"),
        hasPrivateKey: Boolean(process.env.TAURI_SIGNING_PRIVATE_KEY?.trim()),
      })
      console.log("OneHall signed update configuration is explicit")
    } else {
      if (process.argv.length !== 2)
        throw new Error("Unknown collector arguments")
      const env = process.env
      if (!["true", "false"].includes(env.SIGNED_UPDATES ?? "false")) {
        throw new Error("SIGNED_UPDATES must be true or false")
      }
      const baseConfig = JSON.parse(
        readFileSync("src-tauri/tauri.conf.json", "utf8")
      )
      const { version } = baseConfig
      const releaseIdentity =
        env.SIGNED_UPDATES === "true"
          ? captureReleaseIdentity({
              policy: JSON.parse(readFileSync("onehall.release.json", "utf8")),
              baseConfig,
              hallConfig: JSON.parse(
                readFileSync("src-tauri/tauri.hall.conf.json", "utf8")
              ),
            })
          : undefined
      collectDesktopArtifacts({
        bundleRoot: join(
          "src-tauri/target",
          env.BUILD_TARGET,
          "release/bundle"
        ),
        outputDir: "dist-hall",
        platform: env.BUILD_PLATFORM,
        target: env.BUILD_TARGET,
        bundles: env.BUILD_BUNDLES,
        signedUpdates: env.SIGNED_UPDATES === "true",
        releaseIdentity,
        version,
        commit: env.GITHUB_SHA,
        run: env.GITHUB_RUN_ID,
      })
      console.log("Collected desktop artifacts and provenance")
    }
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
