const { authenticate } = require('./auth');

async function main() {
  try {
    console.log('Starting authentication process...');
    const token = await authenticate();
    console.log('Authentication successful:', token);
  } catch (error) {
    console.error('Authentication process failed:', error.message);
    process.exit(1);
  }
}

main();
