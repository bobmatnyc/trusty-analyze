<script>
  /*
   * Why: Operators need to see which indexes trusty-search has registered
   * and pick one as the analysis target.
   * What: Lists indexes proxied from trusty-search, with a Select button
   * that updates the global selectedIndex and pre-loads quality/hotspots
   * for that index.
   * Test: Click Select on any index, verify Dashboard now shows its stats.
   */
  import {
    getIndexes,
    getSelectedIndex,
    setSelectedIndex,
    refreshQuality,
    refreshHotspots,
    refreshSmells,
    refreshIndexes
  } from '../state.svelte.js';
  import { navigate } from '../router.svelte.js';
  import { onMount } from 'svelte';

  let indexes = $derived(getIndexes());
  let selected = $derived(getSelectedIndex());
  let loadError = $state(null);
  let loadingId = $state('');

  onMount(() => {
    refreshIndexes().catch((e) => {
      loadError = e.message || String(e);
    });
  });

  async function selectIndex(id) {
    setSelectedIndex(id);
    loadingId = id;
    loadError = null;
    try {
      await Promise.all([
        refreshQuality(id),
        refreshHotspots(id, 20),
        refreshSmells(id)
      ]);
      navigate('/');
    } catch (e) {
      loadError = e.message || String(e);
    } finally {
      loadingId = '';
    }
  }
</script>

<div>
  <h1 class="page-title">Indexes</h1>

  {#if loadError}
    <div class="card mb-4" style="border-color: var(--trusty-danger)">
      <div class="card-body" style="color: var(--trusty-danger)">
        {loadError}
      </div>
    </div>
  {/if}

  <div class="card">
    <div class="card-header">Registered Indexes</div>
    <div class="card-body" style="padding: 0">
      {#if !indexes || indexes.length === 0}
        <div class="empty">No indexes registered with trusty-search.</div>
      {:else}
        <table class="table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Root</th>
              <th style="width: 120px">Status</th>
              <th style="width: 140px"></th>
            </tr>
          </thead>
          <tbody>
            {#each indexes as idx}
              {@const id = idx.id || idx.name}
              <tr>
                <td class="text-mono">{id}</td>
                <td class="text-mono truncate" title={idx.root_path || idx.path}>
                  {idx.root_path || idx.path || '—'}
                </td>
                <td>
                  {#if id === selected}
                    <span class="badge badge-success">selected</span>
                  {:else}
                    <span class="badge badge-muted">available</span>
                  {/if}
                </td>
                <td>
                  <button
                    class="btn btn-sm"
                    class:btn-primary={id !== selected}
                    disabled={loadingId === id}
                    onclick={() => selectIndex(id)}
                  >
                    {#if loadingId === id}
                      Loading…
                    {:else if id === selected}
                      Reload
                    {:else}
                      Select
                    {/if}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
</div>

<style>
  .page-title {
    font-size: var(--trusty-fs-xl);
    font-weight: 600;
    margin: 0 0 var(--trusty-space-4) 0;
  }
</style>
