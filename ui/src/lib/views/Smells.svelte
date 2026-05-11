<script>
  /*
   * Why: Code smells are the actionable surface of the analysis pipeline —
   * operators want to filter by category and jump to the offending file.
   * What: Renders a category filter dropdown and a smells table. Calls
   * /indexes/:id/smells with the optional category query parameter.
   * Test: Pick a category from the dropdown, confirm table refreshes with
   * only that category's rows.
   */
  import { onMount } from 'svelte';
  import {
    getSelectedIndex,
    getSmells,
    refreshSmells
  } from '../state.svelte.js';

  let selected = $derived(getSelectedIndex());
  let smells = $derived(getSmells());
  let category = $state('');
  let loading = $state(false);
  let error = $state(null);

  async function load() {
    if (!selected) return;
    loading = true;
    error = null;
    try {
      await refreshSmells(selected, category || undefined);
    } catch (e) {
      error = e.message || String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if (selected) load();
  });

  let categories = $derived.by(() => {
    const set = new Set();
    for (const s of smells || []) {
      const c = s.category || s.kind;
      if (c) set.add(c);
    }
    return Array.from(set).sort();
  });

  function onCategoryChange(e) {
    category = e.target.value;
    load();
  }
</script>

<div>
  <h1 class="page-title">Smells</h1>

  {#if !selected}
    <div class="card">
      <div class="card-body empty">No index selected. Choose one from Indexes.</div>
    </div>
  {:else}
    <div class="flex-between mb-4">
      <div class="text-muted text-sm">
        Index: <span class="text-mono">{selected}</span>
      </div>
      <div class="flex-gap-2">
        <select class="select" value={category} onchange={onCategoryChange}>
          <option value="">All categories</option>
          {#each categories as c}
            <option value={c}>{c}</option>
          {/each}
        </select>
        <button class="btn" onclick={load} disabled={loading}>
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>
    </div>

    {#if error}
      <div class="card mb-4" style="border-color: var(--trusty-danger)">
        <div class="card-body" style="color: var(--trusty-danger)">
          {error}
        </div>
      </div>
    {/if}

    <div class="card">
      <div class="card-header">Detected Smells {smells?.length ? `(${smells.length})` : ''}</div>
      <div class="card-body" style="padding: 0">
        {#if loading}
          <div class="empty">Loading…</div>
        {:else if !smells || smells.length === 0}
          <div class="empty">No smells detected.</div>
        {:else}
          <table class="table">
            <thead>
              <tr>
                <th style="width: 160px">Category</th>
                <th>File</th>
                <th>Function</th>
                <th>Detail</th>
              </tr>
            </thead>
            <tbody>
              {#each smells as s}
                <tr>
                  <td>
                    <span class="badge badge-warning">
                      {s.category || s.kind || '—'}
                    </span>
                  </td>
                  <td class="text-mono truncate" title={s.file || s.path}>
                    {s.file || s.path || '—'}
                  </td>
                  <td class="text-mono">{s.function || s.symbol || '—'}</td>
                  <td class="text-sm">{s.message || s.detail || s.description || '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .page-title {
    font-size: var(--trusty-fs-xl);
    font-weight: 600;
    margin: 0 0 var(--trusty-space-4) 0;
  }
</style>
