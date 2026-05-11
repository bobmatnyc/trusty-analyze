<script>
  /*
   * Why: Shell layout that mirrors trusty-search — fixed dark sidebar on
   * the left, sticky topbar, and a hash-routed content pane that renders one
   * of five views.
   * What: Bootstraps the centralized state (health + indexes), then
   * dispatches the route to Dashboard / Indexes / Smells / Facts / Config.
   * Test: Open /ui in a browser, verify nav items render and the
   * version badge turns green once /health responds.
   */
  import Sidebar from './lib/components/Sidebar.svelte';
  import Topbar from './lib/components/Topbar.svelte';
  import Dashboard from './lib/views/Dashboard.svelte';
  import Indexes from './lib/views/Indexes.svelte';
  import Smells from './lib/views/Smells.svelte';
  import Facts from './lib/views/Facts.svelte';
  import Config from './lib/views/Config.svelte';
  import { getRoute } from './lib/router.svelte.js';
  import { refreshHealth, refreshIndexes } from './lib/state.svelte.js';
  import { onMount } from 'svelte';

  let bootError = $state(null);

  onMount(() => {
    refreshHealth().catch((e) => {
      bootError = e.message || String(e);
    });
    refreshIndexes().catch(() => {});
    // Poll /health every 10s so the version badge stays live.
    const t = setInterval(() => {
      refreshHealth().catch(() => {});
    }, 10_000);
    return () => clearInterval(t);
  });

  let route = $derived(getRoute());

  let view = $derived.by(() => {
    const segs = route.segments;
    if (segs.length === 0) return { kind: 'dashboard' };
    if (segs[0] === 'indexes' || segs[0] === 'index') return { kind: 'indexes' };
    if (segs[0] === 'smells') return { kind: 'smells' };
    if (segs[0] === 'facts') return { kind: 'facts' };
    if (segs[0] === 'config') return { kind: 'config' };
    return { kind: 'dashboard' };
  });
</script>

<div class="layout">
  <Sidebar />
  <div class="main">
    <Topbar />
    <div class="content">
      {#if bootError}
        <div class="card" style="border-color: var(--trusty-danger)">
          <div class="card-header" style="color: var(--trusty-danger)">
            Connection error
          </div>
          <div class="card-body">
            <p>{bootError}</p>
            <p class="text-muted text-sm">
              Make sure trusty-analyzer is running with
              <code>trusty-analyzer serve</code> and that trusty-search is
              reachable.
            </p>
          </div>
        </div>
      {:else if view.kind === 'dashboard'}
        <Dashboard />
      {:else if view.kind === 'indexes'}
        <Indexes />
      {:else if view.kind === 'smells'}
        <Smells />
      {:else if view.kind === 'facts'}
        <Facts />
      {:else if view.kind === 'config'}
        <Config />
      {/if}
    </div>
  </div>
</div>

<style>
  .layout {
    display: flex;
    min-height: 100vh;
  }
  .main {
    flex: 1;
    display: flex;
    flex-direction: column;
    margin-left: var(--trusty-sidebar-width);
    min-width: 0;
  }
  .content {
    padding: var(--trusty-space-5) var(--trusty-space-6);
    flex: 1;
    min-width: 0;
  }
</style>
