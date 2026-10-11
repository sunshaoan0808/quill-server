/**
 * src/js/tavern.js — Story Tavern domain entry (real ES module).
 *
 * [2026-10-04 mechanical split] The implementation (6.4k lines, 268 top-level
 * statements) now lives in ./tavern/parts/p001..p008.js, concatenated at build
 * time by vite.config.mjs's tavernPartsVirtual() into the 'virtual:tavern'
 * module. Parts share ONE module scope (same-module concatenation, the
 * appPartsVirtual precedent), so the canonical lets / export-let live bindings
 * behave exactly as before. This file stays the stable import surface:
 * consumers keep importing './tavern.js'; the virtual module is an
 * implementation detail. Parts are verbatim byte slices of the pre-split file
 * (asserted by the cutter), so no statement was edited.
 */
export {
  tavernSession, tavernPack, stCurrentSession, stCurrentPack, stSwipe, initTavernUI,
  stApi, stStatus, stGoBack, stSwitchView, stDisplayTitle, stHasOpenOverlay, stBindImmChrome,
  stRefresh, stLoadPacks, stLoadSessions, stLoadSaves, stLoadSession, stRefreshCharSummary,
  stRenderContinueCard, renderHomeRecent, loadBookshelf, stTurnLabel, stShowPack,
} from 'virtual:tavern';
