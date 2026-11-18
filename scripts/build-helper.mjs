// Membangun oncode-helper dan menyalinnya ke src-tauri/binaries dengan akhiran
// target triple, seperti yang diwajibkan Tauri untuk binary pendamping (sidecar).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const release = process.argv.includes("--release");
const targetArg = process.argv.find((a) => a.startsWith("--target="));
const hostTriple = /host: (\S+)/.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))?.[1];
const triple = targetArg ? targetArg.slice("--target=".length) : hostTriple;
if (!triple) throw new Error("Tidak bisa menentukan target triple dari `rustc -vV`");

const args = ["build", "-p", "oncode-helper"];
if (release) args.push("--release");
if (targetArg) args.push("--target", triple);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const ext = triple.includes("windows") ? ".exe" : "";
const profile = release ? "release" : "debug";
const built = join(root, "target", ...(targetArg ? [triple] : []), profile, `oncode-helper${ext}`);
const outDir = join(root, "src-tauri", "binaries");
mkdirSync(outDir, { recursive: true });
const out = join(outDir, `oncode-helper-${triple}${ext}`);
copyFileSync(built, out);
console.log(`oncode-helper → ${out}`);
