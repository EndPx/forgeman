const logger = require('./logger');

/**
 * Executes an async request with exponential backoff and retry logic.
 * @param {Function} requestFn - An async function that returns a Promise.
 * @param {number} maxAttempts - Maximum number of retry attempts (default: 3).
 * @param {number} initialDelayMs - Initial delay in milliseconds (default: 1000).
 * @param {number} maxDelayMs - Maximum delay in milliseconds to cap exponential growth (default: 10000).
 * @returns {Promise} - The result of the request or throws the final error.
 */
async function makeRequestWithRetry(requestFn, maxAttempts = 3, initialDelayMs = 1000, maxDelayMs = 10000) {
  let lastError;
  let currentDelay = initialDelayMs;

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      const result = await requestFn();
      return result;
    } catch (error) {
      lastError = error;
      const isRateLimit = error.response && error.response.status === 429;

      // If it's the last attempt or not a retryable error (like 401), throw immediately
      if (attempt === maxAttempts || !isRateLimit) {
        if (isRateLimit) {
          logger.warn(`Authentication failed after ${maxAttempts} attempts due to rate limiting: ${error.message}`);
        } else {
          logger.error(`Authentication failed permanently: ${error.message}`);
        }
        throw error;
      }

      // Calculate exponential backoff delay
      const delay = Math.min(currentDelay, maxDelayMs);
      logger.info(`Rate limited. Retrying in ${delay}ms (attempt ${attempt}/${maxAttempts})...`);

      await new Promise(resolve => setTimeout(resolve, delay));

      // Exponential growth: double the delay for the next retry
      currentDelay *= 2;
    }
  }

  // Fallback throw
  throw lastError;
}

module.exports = {
  makeRequestWithRetry
};