const { execSync } = require('child_process');
const path = require('path');

const COMPOSE_FILE = path.join(__dirname, 'docker-compose.test.yml');

module.exports = async function globalTeardown() {
  console.log('[teardown] Stopping test postgres...');
  execSync(`docker compose -f ${COMPOSE_FILE} down -v`, { stdio: 'inherit' });
};
