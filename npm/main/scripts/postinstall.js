const setup = require('../lib/setup');

// Silent mode detection
const silent = process.env.npm_config_loglevel === 'silent' ||
               process.env.CCLINE_SKIP_POSTINSTALL === '1';

function log(message) {
  if (!silent) {
    console.log(message);
  }
}

log('🚀 Setting up CCometixLine for Claude Code...');

try {
  const found = setup.findBinary();
  if (!found.packageName) {
    log(`Platform ${found.platformKey} not supported for auto-setup`);
    process.exit(0);
  }
  if (!found.path) {
    log('Binary package not installed, skipping Claude Code setup');
    log('The global ccline command will still work via npm');
    process.exit(0);
  }

  setup.install(found.path);

  log('✨ CCometixLine is ready for Claude Code!');
  log(`📍 Location: ${setup.targetPath}`);
  log('🎉 You can now use: ccline --help');
} catch (error) {
  // Don't break installation; the launcher retries on its next run
  log('Note: Could not auto-configure for Claude Code');
  log(`Reason: ${error.message}`);
  log('Run ccline once to retry, or copy ccline to ~/.claude/ccline/ yourself');
}
