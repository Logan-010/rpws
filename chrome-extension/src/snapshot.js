export function scheduleSnapshot(callback, delay) {
  const worker = new Worker(new URL('./snapshot-worker.js', import.meta.url), { type: 'module' });
  worker.onmessage = () => callback();
  worker.postMessage(delay);
  return worker;
}
