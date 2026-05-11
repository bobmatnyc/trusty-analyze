<script>
  /*
   * Why: Breadcrumb + daemon-status header. The status dot reflects whether
   * trusty-search (the hard runtime dependency) is reachable from the
   * analyzer.
   * What: Renders crumbs derived from the current route, then a right-side
   * cluster with the reachability dot + version badge.
   * Test: Stop trusty-search, refresh /health, confirm dot turns red.
   */
  import { getHealth } from '../state.svelte.js';
  import { getRoute } from '../router.svelte.js';

  let health = $derived(getHealth());
  let route = $derived(getRoute());

  let crumbs = $derived.by(() => {
    const segs = route.segments;
    if (segs.length === 0) return ['Dashboard'];
    if (segs[0] === 'indexes' || segs[0] === 'index') {
      const parts = ['Indexes'];
      if (segs.length > 1) parts.push(segs[1]);
      return parts;
    }
    if (segs[0] === 'smells') return ['Smells'];
    if (segs[0] === 'facts') return ['Facts'];
    if (segs[0] === 'config') return ['Config'];
    return ['Dashboard'];
  });

  let healthy = $derived(health && health.status === 'ok');
  let searchReachable = $derived(health && health.search_reachable === true);
</script>

<header class="topbar">
  <div class="crumbs">
    {#each crumbs as crumb, i}
      {#if i > 0}<span class="sep">/</span>{/if}
      <span class="crumb">{crumb}</span>
    {/each}
  </div>
  <div class="actions">
    <div class="status" title={searchReachable ? 'trusty-search reachable' : 'trusty-search unreachable'}>
      <span class="dot" class:on={searchReachable} class:off={health && !searchReachable}></span>
      <span class="text-xs text-muted">search</span>
    </div>
    {#if health && healthy}
      <span class="badge badge-success">v{health.version || '?'}</span>
    {:else if health}
      <span class="badge badge-danger">unreachable</span>
    {:else}
      <span class="badge badge-muted">connecting…</span>
    {/if}
  </div>
</header>

<style>
  .topbar {
    height: var(--trusty-topbar-height);
    background: var(--trusty-card-bg);
    border-bottom: 1px solid var(--trusty-border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 var(--trusty-space-6);
    position: sticky;
    top: 0;
    z-index: 10;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--trusty-fs-sm);
    color: var(--trusty-text-secondary);
  }
  .crumb {
    font-weight: 500;
  }
  .crumb:last-child {
    color: var(--trusty-text-primary);
    font-weight: 600;
  }
  .sep {
    color: var(--trusty-text-muted);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--trusty-text-muted);
    display: inline-block;
  }
  .dot.on {
    background: var(--trusty-success);
  }
  .dot.off {
    background: var(--trusty-danger);
  }
</style>
