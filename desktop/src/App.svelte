<script lang="ts">
  import {
    listRequests,
    readRequest,
    runCollection,
    type RequestInfo,
    type ExecutionResult,
  } from "./lib/api";

  let path = $state("");
  let env = $state("");
  let requests = $state<RequestInfo[]>([]);
  let results = $state<ExecutionResult[]>([]);
  let selected = $state<string | null>(null);
  let source = $state("");
  let error = $state<string | null>(null);
  let running = $state(false);

  async function load() {
    error = null;
    results = [];
    try {
      requests = await listRequests(path);
    } catch (e) {
      error = String(e);
    }
  }

  async function run() {
    error = null;
    running = true;
    try {
      results = await runCollection(path, env || undefined);
    } catch (e) {
      error = String(e);
    } finally {
      running = false;
    }
  }

  async function open(p: string) {
    selected = p;
    try {
      source = await readRequest(p);
    } catch (e) {
      source = String(e);
    }
  }

  function resultFor(name: string): ExecutionResult | undefined {
    return results.find((r) => r.request_name === name);
  }

  const counts = $derived({
    ok: results.filter((r) => r.status === "ok").length,
    failed: results.filter((r) => r.status === "failed").length,
    error: results.filter((r) => r.status === "error").length,
  });
</script>

<main>
  <header>
    <h1>protoglot</h1>
    <div class="bar">
      <input class="grow" placeholder="collection path…" bind:value={path} />
      <input class="env" placeholder="env" bind:value={env} />
      <button onclick={load}>Load</button>
      <button onclick={run} disabled={running || requests.length === 0}>
        {running ? "Running…" : "Run all"}
      </button>
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    {#if results.length}
      <p class="summary">
        <span class="ok">{counts.ok} passed</span>,
        <span class="failed">{counts.failed} failed</span>,
        <span class="errored">{counts.error} errored</span>
      </p>
    {/if}
  </header>

  <div class="cols">
    <ul class="list">
      {#each requests as r (r.path)}
        {@const res = resultFor(r.name)}
        <li class:active={selected === r.path} onclick={() => open(r.path)}>
          <span class="dot {res?.status ?? 'none'}"></span>
          <span class="name">{r.name}</span>
          <span class="kind">{r.kind}</span>
        </li>
      {/each}
      {#if requests.length === 0}
        <li class="empty">Load a collection to see its requests.</li>
      {/if}
    </ul>

    <section class="detail">
      {#if selected}
        <h2>Source</h2>
        <pre class="source">{source}</pre>
      {/if}

      {#if results.length}
        <h2>Results</h2>
        {#each results as r (r.request_name)}
          <div class="result {r.status}">
            <div class="result-head">
              <strong>{r.request_name}</strong>
              <em>{r.protocol}</em>
              {#if r.response}<span class="status">{r.response.status}</span>{/if}
              <span class="ms">{r.duration_ms}ms</span>
            </div>
            {#if r.error}<p class="error">{r.error}</p>{/if}
            <ul class="assertions">
              {#each r.assertions as a (a.description)}
                <li class:fail={!a.passed}>
                  {a.passed ? "✓" : "✗"}
                  {a.description}{#if a.message} — {a.message}{/if}
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      {/if}
    </section>
  </div>
</main>
