#!/usr/bin/env node
const { spawnSync } = require('child_process');
const setup = require('../lib/setup');

function run(binary) {
  const result = spawnSync(binary, process.argv.slice(2), {
    stdio: 'inherit',
    shell: false
  });
  process.exit(result.status || 0);
}

// 1. Priority: Use ~/.claude/ccline/ccline when it is this version or was put
// there by hand
const state = setup.targetState();
if (state === 'current' || state === 'manual') {
  run(setup.targetPath);
}

// 2. Otherwise install this version there from the npm package, since npm may
// not have run postinstall
const found = setup.findBinary();
if (!found.packageName) {
  console.error(`Error: Unsupported platform ${found.platformKey}`);
  console.error('Supported platforms: darwin (x64/arm64), linux (x64/arm64), win32 (x64)');
  console.error('Please visit https://github.com/IpiggyI/CCometixLine for manual installation');
  process.exit(1);
}
if (!found.path) {
  console.error(`Error: Binary not found for ${found.packageName}`);
  console.error('This might indicate a failed installation or unsupported platform.');
  console.error('Please try reinstalling: npm install -g @ipiggyi/ccline');
  process.exit(1);
}

try {
  setup.install(found.path);
  console.error(`ccline ${setup.VERSION} installed at ${setup.targetPath}`);
  run(setup.targetPath);
} catch (error) {
  console.error(`ccline: could not install to ${setup.targetPath}: ${error.message}`);
  run(found.path);
}
