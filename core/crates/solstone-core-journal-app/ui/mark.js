// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The journal's mark: two tinted glyph chips and two words. The card is the
// one the journal's own pages show; the tile is the app icon, drawn to the
// same geometry as the Mac app's icon so one journal looks the same on both.

const SVG_NS = 'http://www.w3.org/2000/svg';
const GENERIC = {
  chips: ['#E8913A', '#D4A017'],
  words: ['your', 'journal'],
};
const PLATE_FILL = '#FCF3E4';
const WORD_INK = '#3F3830';
const PLATE_PATH = 'M 527.36 0 c 103.834 0 155.751 0 195.41 20.207 a 185.4 185.4 0 0 1 81.023 81.023 c 20.207 39.659 20.207 91.576 20.207 195.41 L 824 527.36 c 0 103.834 0 155.751 -20.207 195.41 a 185.4 185.4 0 0 1 -81.023 81.023 c -39.659 20.207 -91.576 20.207 -195.41 20.207 L 296.64 824 c -103.834 0 -155.751 0 -195.41 -20.207 a 185.4 185.4 0 0 1 -81.023 -81.023 c -20.207 -39.659 -20.207 -91.576 -20.207 -195.41 L 0 296.64 c 0 -103.834 0 -155.751 20.207 -195.41 a 185.4 185.4 0 0 1 81.023 -81.023 c 39.659 -20.207 91.576 -20.207 195.41 -20.207 Z';

function validMark(mark) {
  if (!mark || !Array.isArray(mark.words) || mark.words.length !== 2) return false;
  return [mark.icon1, mark.icon2].every(icon =>
    icon && typeof icon.svg === 'string' && icon.svg.trim() &&
    icon.color && /^#[0-9a-fA-F]{6}$/.test(icon.color.hex) &&
    (icon.rot === 0 || icon.rot === 45));
}

// The glyph markup the journal serves, read as shapes rather than inserted
// as markup: only the drawing elements a glyph uses are kept.
function glyphShapes(svg) {
  const doc = new DOMParser().parseFromString(
    `<svg xmlns="${SVG_NS}">${svg}</svg>`, 'image/svg+xml');
  if (doc.querySelector('parsererror')) return [];
  const allowed = new Set(['path', 'circle', 'rect', 'line', 'polyline', 'polygon', 'ellipse']);
  return [...doc.documentElement.children].filter(el => allowed.has(el.localName));
}

function glyphSvgElement(svg, hex) {
  const el = document.createElementNS(SVG_NS, 'svg');
  el.setAttribute('viewBox', '0 0 24 24');
  el.setAttribute('fill', 'none');
  el.setAttribute('stroke', hex);
  el.setAttribute('stroke-width', '2');
  el.setAttribute('stroke-linecap', 'round');
  el.setAttribute('stroke-linejoin', 'round');
  el.setAttribute('aria-hidden', 'true');
  for (const shape of glyphShapes(svg)) {
    const copy = document.createElementNS(SVG_NS, shape.localName);
    for (const name of ['d', 'cx', 'cy', 'r', 'rx', 'ry', 'x', 'y', 'width', 'height', 'x1', 'y1', 'x2', 'y2', 'points']) {
      if (shape.hasAttribute(name)) copy.setAttribute(name, shape.getAttribute(name));
    }
    el.appendChild(copy);
  }
  return el;
}

// The mark card. With no mark yet it shows the journal's "no identity yet"
// treatment: dashed, empty chips and "your · journal", never an empty box.
export function renderMarkCard(container, mark, confirmedLine) {
  container.replaceChildren();
  container.classList.add('mark-card');
  const chips = document.createElement('div');
  chips.className = 'mark-chips';
  const words = document.createElement('div');
  words.className = 'mark-words';
  const known = validMark(mark);
  if (known) {
    for (const icon of [mark.icon1, mark.icon2]) {
      const chip = document.createElement('div');
      chip.className = 'mark-chip';
      chip.style.borderColor = icon.color.hex;
      chip.style.background = icon.color.hex + '1f';
      if (icon.rot === 45) chip.classList.add('rotated');
      chip.appendChild(glyphSvgElement(icon.svg, icon.color.hex));
      chips.appendChild(chip);
    }
    words.textContent = mark.words.join(' · ');
    container.setAttribute('aria-label',
      `${mark.icon1.color.name || ''}, ${mark.icon2.color.name || ''} · ${mark.words.join(' ')}`);
  } else {
    GENERIC.chips.forEach((hex, index) => {
      const chip = document.createElement('div');
      chip.className = 'mark-chip generic';
      chip.style.borderColor = hex;
      chip.style.background = hex + '12';
      if (index === 1) chip.classList.add('rotated');
      chips.appendChild(chip);
    });
    words.textContent = GENERIC.words.join(' · ');
    container.setAttribute('aria-label', 'your journal, not set up yet');
  }
  container.setAttribute('role', 'img');
  container.classList.toggle('confirmed', Boolean(known && confirmedLine));
  container.append(chips, words);
  if (known && confirmedLine) {
    const line = document.createElement('div');
    line.className = 'mark-confirmed';
    line.textContent = confirmedLine;
    container.appendChild(line);
  }
}

// --- the icon tile -------------------------------------------------------

function roundedSquare(ctx, cx, cy, side, radius, degrees) {
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(degrees * Math.PI / 180);
  ctx.beginPath();
  ctx.roundRect(-side / 2, -side / 2, side, side, radius);
  ctx.restore();
}

function chipLayout(tile, rotations) {
  const side = tile * 0.27;
  const gap = side * 0.23;
  const cy = tile * 0.34;
  const separation = side + gap;
  const centers = [tile / 2 - separation / 2, tile / 2 + separation / 2];
  const half = rotations.map(deg => {
    const r = Math.abs(deg) * Math.PI / 180;
    return side * (Math.abs(Math.cos(r)) + Math.abs(Math.sin(r))) / 2;
  });
  const left = centers[0] - half[0];
  const right = centers[1] + half[1];
  const shift = tile / 2 - (left + right) / 2;
  return { side, cy, centers: centers.map(x => x + shift) };
}

function strokeShape(ctx, shape) {
  const n = name => parseFloat(shape.getAttribute(name) || '0');
  const path = new Path2D();
  switch (shape.localName) {
    case 'path': path.addPath(new Path2D(shape.getAttribute('d') || '')); break;
    case 'circle': path.arc(n('cx'), n('cy'), n('r'), 0, Math.PI * 2); break;
    case 'ellipse': path.ellipse(n('cx'), n('cy'), n('rx'), n('ry'), 0, 0, Math.PI * 2); break;
    case 'rect': path.roundRect(n('x'), n('y'), n('width'), n('height'), n('rx') || n('ry') || 0); break;
    case 'line': path.moveTo(n('x1'), n('y1')); path.lineTo(n('x2'), n('y2')); break;
    case 'polyline':
    case 'polygon': {
      const points = (shape.getAttribute('points') || '').trim().split(/[\s,]+/).map(Number);
      for (let i = 0; i + 1 < points.length; i += 2) {
        if (i === 0) path.moveTo(points[i], points[i + 1]); else path.lineTo(points[i], points[i + 1]);
      }
      if (shape.localName === 'polygon') path.closePath();
      break;
    }
  }
  ctx.stroke(path);
}

function drawWords(ctx, tile, words) {
  const lower = words.map(word => word.trim().toLowerCase());
  const scale = tile / 100;
  ctx.font = `700 ${12 * scale}px Comfortaa`;
  const widest = Math.max(...lower.map(word => ctx.measureText(word).width)) / scale;
  const size = widest > 76 ? 12 * 76 / widest : 12;
  ctx.font = `700 ${size * scale}px Comfortaa`;
  ctx.fillStyle = WORD_INK;
  ctx.textAlign = 'center';
  ctx.textBaseline = 'alphabetic';
  ctx.fillText(lower[0], tile / 2, tile * 0.70);
  ctx.fillText(lower[1], tile / 2, tile * 0.86);
}

async function drawTile(tile, mark) {
  await document.fonts.load('700 12px Comfortaa');
  const canvas = document.createElement('canvas');
  canvas.width = tile;
  canvas.height = tile;
  const ctx = canvas.getContext('2d');
  ctx.save();
  ctx.setTransform(tile / 1024, 0, 0, tile / 1024, 100 * tile / 1024, 100 * tile / 1024);
  ctx.fillStyle = PLATE_FILL;
  ctx.fill(new Path2D(PLATE_PATH));
  ctx.restore();

  const known = validMark(mark);
  const icons = known ? [mark.icon1, mark.icon2] : null;
  const rotations = known ? icons.map(icon => icon.rot) : [0, 45];
  const layout = chipLayout(tile, rotations);
  const radius = layout.side * 0.25;
  const border = 2 * layout.side / 48;
  layout.centers.forEach((cx, index) => {
    const rotation = rotations[index];
    const hex = known ? icons[index].color.hex : GENERIC.chips[index];
    ctx.save();
    roundedSquare(ctx, cx, layout.cy, layout.side, radius, rotation);
    ctx.globalAlpha = known ? 0.12 : 0.07;
    ctx.fillStyle = hex;
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.strokeStyle = hex;
    ctx.lineWidth = border;
    if (!known) ctx.setLineDash([3.2 * layout.side / 27, 2.4 * layout.side / 27]);
    ctx.stroke();
    ctx.restore();
    if (known) {
      const glyph = layout.side * 0.58;
      ctx.save();
      ctx.translate(cx, layout.cy);
      ctx.rotate(rotation * Math.PI / 180);
      ctx.translate(-glyph / 2, -glyph / 2);
      ctx.scale(glyph / 24, glyph / 24);
      ctx.strokeStyle = hex;
      ctx.lineWidth = Math.max(2, 24 / glyph);
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      for (const shape of glyphShapes(icons[index].svg)) strokeShape(ctx, shape);
      ctx.restore();
    }
  });
  drawWords(ctx, tile, known ? mark.words : GENERIC.words);
  return canvas;
}

// The icon images for the app, one PNG per size Windows asks for.
export async function markIconImages(mark, sides) {
  const images = [];
  for (const side of sides) {
    const canvas = await drawTile(side, mark);
    const blob = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'));
    images.push([side, Array.from(new Uint8Array(await blob.arrayBuffer()))]);
  }
  return images;
}

export function sameMark(left, right) {
  return validMark(left) && validMark(right) && JSON.stringify(left) === JSON.stringify(right);
}

export { validMark };
