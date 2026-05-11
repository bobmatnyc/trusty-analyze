/*
 * Why: Centralized module-level reactive state shared by every view, so a
 * single fetch of /health or /indexes is reused across the dashboard,
 * smells, and facts panes. Persisting selectedIndex in localStorage keeps
 * context when the analyst reloads the page.
 * What: $state primitives + getter functions + refresh helpers calling the
 * analyzer HTTP API.
 * Test: Call refreshHealth() in console, then getHealth() — assert non-null.
 */
import { api } from './api.js';

let _health = $state(null);
let _indexes = $state([]);
let _selectedIndex = $state(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem('trusty-analyzer.selectedIndex') || ''
    : ''
);
let _quality = $state(null);
let _hotspots = $state([]);
let _smells = $state([]);
let _facts = $state([]);

export function getHealth() {
  return _health;
}
export function getIndexes() {
  return _indexes;
}
export function getSelectedIndex() {
  return _selectedIndex;
}
export function getQuality() {
  return _quality;
}
export function getHotspots() {
  return _hotspots;
}
export function getSmells() {
  return _smells;
}
export function getFacts() {
  return _facts;
}

export function setSelectedIndex(id) {
  _selectedIndex = id;
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('trusty-analyzer.selectedIndex', id);
  }
}

export async function refreshHealth() {
  _health = await api.health();
  return _health;
}
export async function refreshIndexes() {
  _indexes = await api.indexes();
  return _indexes;
}
export async function refreshQuality(id) {
  _quality = await api.quality(id);
  return _quality;
}
export async function refreshHotspots(id, topK = 20) {
  _hotspots = await api.complexityHotspots(id, topK);
  return _hotspots;
}
export async function refreshSmells(id, category) {
  _smells = await api.smells(id, category);
  return _smells;
}
export async function refreshFacts(subject, predicate) {
  _facts = await api.listFacts(subject, predicate);
  return _facts;
}
