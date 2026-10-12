/**
 * UI blocks public entry — extract fences from text and mount cards.
 * Wiring lives in chat.js fillBubbleBody (same slot as check cards).
 */
import { extractUIBlocks, hashFenceBody, UI_BLOCK_LANGUAGE_DEEIX } from './fence.js';
import { renderUiBlockBody, buildUiBlockError } from './render.js';

export {
  extractUIBlocks,
  hashFenceBody,
  UI_BLOCK_LANGUAGE_DEEIX,
  renderUiBlockBody,
  buildUiBlockError,
};

/** Build the card wrapper for one extracted block. Never throws. */
export function tryMountUiBlock(block) {
  const wrap = document.createElement('div');
  wrap.className = 'st-ui-block';
  wrap.dataset.lang = block.language || '';
  wrap.dataset.hash = block.hash || '';
  let body;
  try {
    body = renderUiBlockBody(block);
  } catch (e) {
    body = buildUiBlockError(e && e.message, block.rawBody);
    wrap.classList.add('is-error');
  }
  wrap.appendChild(body);
  return wrap;
}
