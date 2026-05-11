<script>
  /*
   * Why: Operators want a one-glance view of the daemon's runtime config —
   * version, port, and whether the hard runtime dependency (trusty-search)
   * is reachable.
   * What: Read-only summary derived from /health plus values injected by
   * the daemon at serve time (window.__DAEMON_PORT__, __SEARCH_URL__).
   * Test: Stop trusty-search, refresh, confirm the dot turns red.
   */
  import { getHealth } from '../state.svelte.js';

  let health = $derived(getHealth());
  let reachable = $derived(health && health.search_reachable === true);

  const analyzerPort =
    typeof window !== 'undefined' && window.__DAEMON_PORT__
      ? window.__DAEMON_PORT__
      : 7879;
  const searchUrl =
    typeof window !== 'undefined' && window.__SEARCH_URL__
      ? window.__SEARCH_URL__
      : 'http://127.0.0.1:7878';
</script>

<div>
  <h1 class="page-title">Config</h1>

  <div class="card">
    <div class="card-header">Runtime</div>
    <div class="card-body">
      <dl class="kv">
        <dt>Analyzer Version</dt>
        <dd class="text-mono">{health?.version || '—'}</dd>

        <dt>Analyzer Port</dt>
        <dd class="text-mono">{analyzerPort}</dd>

        <dt>trusty-search URL</dt>
        <dd class="text-mono">{searchUrl}</dd>

        <dt>trusty-search Reachable</dt>
        <dd>
          <span class="dot" class:on={reachable} class:off={health && !reachable}></span>
          <span class="text-sm">
            {#if !health}
              connecting…
            {:else if reachable}
              reachable
            {:else}
              unreachable
            {/if}
          </span>
        </dd>

        <dt>Status</dt>
        <dd>
          {#if health?.status === 'ok'}
            <span class="badge badge-success">ok</span>
          {:else if health}
            <span class="badge badge-danger">{health.status || 'unknown'}</span>
          {:else}
            <span class="badge badge-muted">connecting…</span>
          {/if}
        </dd>
      </dl>
    </div>
  </div>
</div>

<style>
  .page-title {
    font-size: var(--trusty-fs-xl);
    font-weight: 600;
    margin: 0 0 var(--trusty-space-4) 0;
  }
  .kv {
    display: grid;
    grid-template-columns: 220px 1fr;
    row-gap: var(--trusty-space-3);
    column-gap: var(--trusty-space-4);
    margin: 0;
  }
  .kv dt {
    color: var(--trusty-text-muted);
    font-size: var(--trusty-fs-sm);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .kv dd {
    margin: 0;
    color: var(--trusty-text-primary);
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--trusty-text-muted);
    margin-right: 6px;
    vertical-align: middle;
  }
  .dot.on {
    background: var(--trusty-success);
  }
  .dot.off {
    background: var(--trusty-danger);
  }
</style>
