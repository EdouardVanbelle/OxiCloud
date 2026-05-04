const { execSync, spawnSync } = require('child_process');
const net = require('net');
const path = require('path');

const COMPOSE_FILE = path.join(__dirname, 'docker-compose.test.yml');
const CMD = `docker compose -f ${COMPOSE_FILE}`;

function waitForPort(host, port, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    function attempt() {
      const sock = net.connect(port, host);
      sock.once('connect', () => { sock.destroy(); resolve(); });
      sock.once('error', () => {
        sock.destroy();
        if (Date.now() >= deadline) return reject(new Error(`Timeout waiting for ${host}:${port}`));
        setTimeout(attempt, 500);
      });
    }
    attempt();
  });
}

module.exports = async function globalSetup() {
  console.log('[setup] Starting test postgres (database is empty) ...');
  execSync(`${CMD} down`, { stdio: 'inherit' });
  execSync(`${CMD} up -d`, { stdio: 'inherit' });
  console.log('[setup] Waiting for postgres on port 5433...');
  await waitForPort('127.0.0.1', 5433);
  console.log('[setup] Postgres is ready.');
};
