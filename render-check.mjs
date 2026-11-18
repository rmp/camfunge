/** Headless-Chrome smoke test: every shader compiles, every pass draws without a GL error, and motion is detected. */
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const CHROME = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const CAMERA_START = '    startCamera();';
const HARNESS = `
    const feed = Object.assign(document.createElement('canvas'), { width: 640, height: 480 });
    const ink = feed.getContext('2d');
    setInterval(() => {
      ink.fillStyle = '#234'; ink.fillRect(0, 0, 640, 480);
      ink.fillStyle = '#fc9'; ink.fillRect(200 + 150 * Math.sin(performance.now() / 300), 160, 160, 160);
    }, 33);
    video.srcObject = feed.captureStream(30);
    video.play();
    let reported = false;
    const report = result => {
      if (reported) return;
      reported = true;
      document.title = 'RESULT ' + JSON.stringify(result);
    };
    setTimeout(() => report({ failures: ['timed out: frames=' + head + ' readyState=' + video.readyState + ' msg=' + msg.textContent] }), 12000);
    const ready = setInterval(() => {
      if (msg.textContent && msg.textContent !== 'Waking the camera…') return report({ failures: [msg.textContent] });
      if (head < 2) return;
      clearInterval(ready);
      try { check(); } catch (e) { report({ failures: ['harness threw: ' + e.message] }); }
    }, 50);
    function check() {
      const failures = [];
      FILTERS.forEach((f, i) => {
        selectFilter(i);
        frame(performance.now());
        frame(performance.now());
        const err = gl.getError();
        if (err) failures.push(f.name + ': GL error ' + err);
      });
      const w = Math.ceil(canvas.width / MOTION_DOWNSCALE), h = Math.ceil(canvas.height / MOTION_DOWNSCALE);
      const px = new Float32Array(w * h * 4);
      gl.bindFramebuffer(gl.FRAMEBUFFER, motionBuffers[motionRead].fbo);
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.FLOAT, px);
      let maxMotion = 0;
      for (let i = 0; i < px.length; i += 4) maxMotion = Math.max(maxMotion, px[i]);
      if (!(maxMotion > 0.5)) failures.push('motion pass saw no movement (max ' + maxMotion + ')');
      report({ filters: FILTERS.length, failures });
    }
`;

const html = await readFile(new URL('./src/index.html', import.meta.url), 'utf8');
if (!html.includes(CAMERA_START)) throw new Error(`index.html no longer contains "${CAMERA_START.trim()}"`);
const page = html.replace(CAMERA_START, HARNESS);
const reactivity = await readFile(new URL('./src/reactivity.js', import.meta.url), 'utf8');

const server = createServer((req, res) => {
  const js = req.url === '/reactivity.js';
  res.writeHead(200, { 'content-type': js ? 'text/javascript' : 'text/html' });
  res.end(js ? reactivity : page);
}).listen(0);
const url = `http://localhost:${server.address().port}/`;

try {
  const { stdout } = await promisify(execFile)(CHROME, [
    '--headless=new', '--enable-unsafe-swiftshader', '--autoplay-policy=no-user-gesture-required',
    '--window-size=640,400', '--virtual-time-budget=15000', '--dump-dom', url,
  ], { maxBuffer: 1 << 24 });
  const match = stdout.match(/<title>RESULT (.*?)<\/title>/s);
  const pageMessage = stdout.match(/<div id="msg">(.*?)<\/div>/s)?.[1];
  const result = match ? JSON.parse(match[1].replaceAll('&quot;', '"').replaceAll('&amp;', '&')) : { failures: [`no result; page says: ${pageMessage}`] };
  if (result.failures.length) {
    console.error(result.failures.join('\n'));
    process.exitCode = 1;
  } else {
    console.log(`ok: ${result.filters} filters, motion and history passes render`);
  }
} finally {
  server.close();
}
