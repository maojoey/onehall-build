#!/usr/bin/env node
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import {
  existsSync,
  lstatSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs"
import { join } from "node:path"
import {
  captureReleaseIdentity,
  collectDesktopArtifacts,
  DESKTOP_PLATFORMS,
  validateProvenance,
} from "./collect-hall-desktop.mjs"

const args = process.argv.slice(2)
if (
  args[0] !== "--platform" ||
  !DESKTOP_PLATFORMS[args[1]] ||
  args.slice(2).some((arg) => arg !== "--check-only")
)
  throw new Error(
    "Expected --platform mac-x64|mac-arm64|win-x64 [--check-only]"
  )
const platform = args[1]
const checkOnly = args.includes("--check-only")
const spec = DESKTOP_PLATFORMS[platform]
const native = {
  "darwin:x64": "mac-x64",
  "darwin:arm64": "mac-arm64",
  "win32:x64": "win-x64",
}[`${process.platform}:${process.arch}`]
if (native !== platform)
  throw new Error("Runner architecture does not match selected native platform")
const read = (path) => JSON.parse(readFileSync(path, "utf8"))
const manifest = read("public-source.json")
for (const file of manifest.files) {
  if (
    file.path
      .split("/")
      .some((part) => !part || part === "." || part === "..") ||
    file.path.startsWith("/") ||
    file.path.includes("\\")
  )
    throw new Error("Unsafe snapshot path")
  const bytes = readFileSync(file.path)
  if (
    bytes.length !== file.size ||
    createHash("sha256").update(bytes).digest("hex") !== file.sha256
  )
    throw new Error(`Snapshot checksum mismatch: ${file.path}`)
}
const policy = read("onehall.release.json")
const baseConfig = read("src-tauri/tauri.conf.json")
const hallConfig = read("src-tauri/tauri.hall.conf.json")
if (
  policy.version !== manifest.version ||
  baseConfig.version !== manifest.version
)
  throw new Error("Snapshot version mismatch")
const commit = process.env.GITHUB_SHA
const run = process.env.GITHUB_RUN_ID
validateProvenance({ version: manifest.version, commit, run })
const releaseIdentity = captureReleaseIdentity({
  policy,
  baseConfig,
  hallConfig,
})
const exec = (command, argv) =>
  execFileSync(command, argv, {
    stdio: "inherit",
    shell: process.platform === "win32" && command === "pnpm",
  })
const rustHost = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
  /^host: (.+)$/m
)?.[1]
if (rustHost !== spec.target)
  throw new Error("Rust host differs from native target")
if (process.platform === "darwin") exec("xcrun", ["--find", "clang"])
else exec("where.exe", ["cl.exe"])
console.log(
  `Verified snapshot ${manifest.sourceCommit}; public commit ${commit}; native ${platform}`
)
if (!checkOnly) {
  exec("rustup", ["target", "add", spec.target])
  exec("pnpm", ["build"])
  exec(process.execPath, [
    "src-tauri/scripts/prepare-sidecars.mjs",
    "--target",
    spec.target,
  ])
  for (const name of ["codeg-mcp", "codeg-computer-helper", "codeg-cli"]) {
    const stat = lstatSync(
      `src-tauri/binaries/${name}-${spec.target}${process.platform === "win32" ? ".exe" : ""}`
    )
    if (!stat.isFile() || stat.isSymbolicLink() || !stat.size)
      throw new Error("Missing native sidecar")
  }
  writeFileSync(
    "src-tauri/tauri.cloud-build.json",
    JSON.stringify({
      build: { beforeBuildCommand: "" },
      bundle: { createUpdaterArtifacts: false },
    }),
    { flag: "wx" }
  )
  process.env.CODEG_SKIP_SIDECAR = "1"
  exec("pnpm", [
    "tauri",
    "build",
    "--ci",
    "--target",
    spec.target,
    "--bundles",
    spec.bundle === "dmg" ? "app,dmg" : "nsis",
    "--config",
    "src-tauri/tauri.hall.conf.json",
    "--config",
    "src-tauri/tauri.cloud-build.json",
  ])
  const bundleRoot = join("src-tauri/target", spec.target, "release/bundle")
  if (spec.bundle === "dmg") {
    const macos = join(bundleRoot, "macos")
    const apps = readdirSync(macos).filter((name) => name.endsWith(".app"))
    if (apps.length !== 1) throw new Error("Expected exactly one app bundle")
    const archive = join(macos, `${apps[0]}.tar.gz`)
    if (existsSync(archive)) throw new Error("Archive already exists")
    process.env.COPYFILE_DISABLE = "1"
    exec("tar", ["-czf", archive, "-C", macos, "--", apps[0]])
  }
  collectDesktopArtifacts({
    bundleRoot,
    outputDir: "dist-hall",
    platform,
    target: spec.target,
    bundles: spec.bundle,
    prepareUpdates: true,
    releaseIdentity,
    version: manifest.version,
    commit,
    run,
  })
}
