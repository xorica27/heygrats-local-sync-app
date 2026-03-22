import fs from 'node:fs'
import path from 'node:path'

const input = process.argv[2]?.trim() || ''
const version = input.startsWith('v') ? input.slice(1) : input
const versionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z-.]+)?$/

if (!versionPattern.test(version)) {
  console.error(
    'Usage: npm run version:set -- <version>\nExample: npm run version:set -- 0.1.1'
  )
  process.exit(1)
}

const root = process.cwd()

const packageJsonPath = path.join(root, 'package.json')
const packageLockPath = path.join(root, 'package-lock.json')
const cargoTomlPath = path.join(root, 'src-tauri', 'Cargo.toml')
const tauriConfigPath = path.join(root, 'src-tauri', 'tauri.conf.json')

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'))
packageJson.version = version
fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`)

const packageLock = JSON.parse(fs.readFileSync(packageLockPath, 'utf8'))
packageLock.version = version
if (packageLock.packages?.['']) {
  packageLock.packages[''].version = version
}
fs.writeFileSync(packageLockPath, `${JSON.stringify(packageLock, null, 2)}\n`)

const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8').replace(
  /^version = ".*"$/m,
  `version = "${version}"`
)
fs.writeFileSync(cargoTomlPath, cargoToml)

const tauriConfig = JSON.parse(fs.readFileSync(tauriConfigPath, 'utf8'))
tauriConfig.version = version
fs.writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`)

console.log(`Updated app version to ${version}`)
