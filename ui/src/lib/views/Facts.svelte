<script>
  /*
   * Why: FactStore is the (subject, predicate, object) knowledge layer; UI
   * lets operators inspect, search, add, and delete facts without leaving
   * the dashboard.
   * What: Search inputs for subject/predicate, inline add form, and a
   * table with per-row delete. Wraps /facts GET/POST/DELETE.
   * Test: Add a fact, confirm it appears in the table; delete it, confirm
   * row disappears.
   */
  import { onMount } from 'svelte';
  import { getFacts, refreshFacts } from '../state.svelte.js';
  import { api } from '../api.js';

  let facts = $derived(getFacts());
  let subjectQuery = $state('');
  let predicateQuery = $state('');
  let error = $state(null);
  let loading = $state(false);

  // Inline add form state.
  let showForm = $state(false);
  let newSubject = $state('');
  let newPredicate = $state('');
  let newObject = $state('');
  let newProvenance = $state('');
  let saving = $state(false);

  async function load() {
    loading = true;
    error = null;
    try {
      await refreshFacts(subjectQuery || undefined, predicateQuery || undefined);
    } catch (e) {
      error = e.message || String(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  async function submitNew() {
    if (!newSubject || !newPredicate || !newObject) return;
    saving = true;
    error = null;
    try {
      await api.upsertFact({
        subject: newSubject,
        predicate: newPredicate,
        object: newObject,
        provenance: newProvenance || undefined
      });
      newSubject = '';
      newPredicate = '';
      newObject = '';
      newProvenance = '';
      showForm = false;
      await load();
    } catch (e) {
      error = e.message || String(e);
    } finally {
      saving = false;
    }
  }

  async function removeFact(id) {
    if (!id) return;
    if (!confirm('Delete this fact?')) return;
    try {
      await api.deleteFact(id);
      await load();
    } catch (e) {
      error = e.message || String(e);
    }
  }
</script>

<div>
  <h1 class="page-title">Facts</h1>

  <div class="flex-between mb-4">
    <div class="flex-gap-2" style="flex: 1; max-width: 640px">
      <input
        class="input"
        type="text"
        placeholder="Filter by subject"
        bind:value={subjectQuery}
        onkeydown={(e) => e.key === 'Enter' && load()}
      />
      <input
        class="input"
        type="text"
        placeholder="Filter by predicate"
        bind:value={predicateQuery}
        onkeydown={(e) => e.key === 'Enter' && load()}
      />
      <button class="btn" onclick={load} disabled={loading}>
        {loading ? 'Loading…' : 'Search'}
      </button>
    </div>
    <button class="btn btn-primary" onclick={() => (showForm = !showForm)}>
      {showForm ? 'Cancel' : 'Add Fact'}
    </button>
  </div>

  {#if showForm}
    <div class="card mb-4">
      <div class="card-header">New Fact</div>
      <div class="card-body">
        <div class="form-grid">
          <div class="form-group">
            <label class="form-label" for="f-subj">Subject</label>
            <input id="f-subj" class="input" bind:value={newSubject} />
          </div>
          <div class="form-group">
            <label class="form-label" for="f-pred">Predicate</label>
            <input id="f-pred" class="input" bind:value={newPredicate} />
          </div>
          <div class="form-group">
            <label class="form-label" for="f-obj">Object</label>
            <input id="f-obj" class="input" bind:value={newObject} />
          </div>
          <div class="form-group">
            <label class="form-label" for="f-prov">Provenance (optional)</label>
            <input id="f-prov" class="input" bind:value={newProvenance} />
          </div>
        </div>
        <div class="flex-gap-2">
          <button
            class="btn btn-primary"
            disabled={saving || !newSubject || !newPredicate || !newObject}
            onclick={submitNew}
          >
            {saving ? 'Saving…' : 'Save Fact'}
          </button>
          <button class="btn" onclick={() => (showForm = false)}>Cancel</button>
        </div>
      </div>
    </div>
  {/if}

  {#if error}
    <div class="card mb-4" style="border-color: var(--trusty-danger)">
      <div class="card-body" style="color: var(--trusty-danger)">
        {error}
      </div>
    </div>
  {/if}

  <div class="card">
    <div class="card-header">Facts {facts?.length ? `(${facts.length})` : ''}</div>
    <div class="card-body" style="padding: 0">
      {#if loading}
        <div class="empty">Loading…</div>
      {:else if !facts || facts.length === 0}
        <div class="empty">No facts found.</div>
      {:else}
        <table class="table">
          <thead>
            <tr>
              <th>Subject</th>
              <th>Predicate</th>
              <th>Object</th>
              <th>Confidence</th>
              <th>Provenance</th>
              <th style="width: 80px"></th>
            </tr>
          </thead>
          <tbody>
            {#each facts as f}
              <tr>
                <td class="text-mono">{f.subject}</td>
                <td class="text-mono">{f.predicate}</td>
                <td class="text-mono">{f.object}</td>
                <td>{f.confidence != null ? Number(f.confidence).toFixed(2) : '—'}</td>
                <td class="text-sm text-muted truncate" title={f.provenance}>
                  {f.provenance || '—'}
                </td>
                <td>
                  <button class="btn btn-sm btn-danger" onclick={() => removeFact(f.id)}>
                    Delete
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
  .form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--trusty-space-4);
  }
</style>
