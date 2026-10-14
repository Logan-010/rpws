self.onmessage = ({ data: delay }) => {
  setInterval(() => self.postMessage(null), delay);
};
