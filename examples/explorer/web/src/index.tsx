/* @refresh reload */
import { createSignal, Match, Show, Switch, type JSX } from "solid-js";
import { render as mount } from "solid-js/web";
import { QueryClient, QueryClientProvider, createQuery } from "@tanstack/solid-query";

import { api, type Persona, type Sdk } from "./api";
import { Segmented } from "./parts";
import {
  Agreement,
  Batches,
  Explore,
  Groups,
  Joins,
  PredicateWrites,
  Relationships,
  SoftDelete,
  Transactions,
} from "./panels";
import "./styles.css";

const TABS = [
  "rows",
  "joins",
  "groups",
  "relationships",
  "writes",
  "soft delete",
  "batches",
  "transactions",
  "agreement",
] as const;
type Tab = (typeof TABS)[number];

function App(): JSX.Element {
  const [sdk, setSdk] = createSignal<Sdk>("go");
  const [persona, setPersona] = createSignal<Persona>("app");
  const [tab, setTab] = createSignal<Tab>("rows");

  const meta = createQuery(() => ({
    queryKey: ["meta", sdk()],
    queryFn: () => api.meta(sdk(), "app"),
  }));

  const context = { sdk, persona };

  return (
    <div class="shell">
      <header class="top">
        <h1>slate explorer</h1>
        <span class="sub">
          one database, three client libraries, and a flag that swaps between them
        </span>
      </header>

      <div class="bar">
        <Segmented
          label="sdk"
          value={sdk()}
          options={["go", "node", "python"] as const}
          onChange={setSdk}
        />
        <Segmented
          label="identity"
          value={persona()}
          options={["app", "reader", "stranger"] as const}
          onChange={setPersona}
        />
        <Show when={meta.data?.ok && meta.data.value}>
          {(value) => (
            <span class="badge" data-tone={value().leader ? "good" : "warn"}>
              head node <b>{value().leader ? "leader" : "follower"}</b>
            </span>
          )}
        </Show>
      </div>

      <p class="why" style={{ "max-width": "78ch", "margin-top": "-6px" }}>
        The <b>sdk</b> switch changes which client library builds the request —
        nothing else. The <b>identity</b> switch changes who the database thinks
        is asking: <code>reader</code> is subject to a row policy and may not
        ask for a plan, and <code>stranger</code> holds a role with no grant at
        all. None of that is enforced by the adapters.
      </p>

      <div class="tabs">
        {TABS.map((name) => (
          <button type="button" data-on={tab() === name} onClick={() => setTab(name)}>
            {name}
          </button>
        ))}
      </div>

      <Switch>
        <Match when={tab() === "rows"}>
          <Explore {...context} />
        </Match>
        <Match when={tab() === "joins"}>
          <Joins {...context} />
        </Match>
        <Match when={tab() === "groups"}>
          <Groups {...context} />
        </Match>
        <Match when={tab() === "relationships"}>
          <Relationships {...context} />
        </Match>
        <Match when={tab() === "writes"}>
          <PredicateWrites {...context} />
        </Match>
        <Match when={tab() === "soft delete"}>
          <SoftDelete {...context} />
        </Match>
        <Match when={tab() === "batches"}>
          <Batches {...context} />
        </Match>
        <Match when={tab() === "transactions"}>
          <Transactions {...context} />
        </Match>
        <Match when={tab() === "agreement"}>
          <Agreement {...context} />
        </Match>
      </Switch>

      <footer>
        Start the backend with <code>./run.sh --headless</code>. The three
        adapters are in <code>examples/explorer/backends/</code>, one per SDK,
        all implementing <code>CONTRACT.md</code> against the same head node.
      </footer>
    </div>
  );
}

const client = new QueryClient({
  defaultOptions: {
    queries: {
      // A refusal is a result, not a failure: the demo shows "you may not ask"
      // on purpose, and retrying it would only delay the point.
      retry: false,
      refetchOnWindowFocus: false,
      staleTime: 2_000,
    },
  },
});

const root = document.getElementById("root");
if (root) {
  mount(
    () => (
      <QueryClientProvider client={client}>
        <App />
      </QueryClientProvider>
    ),
    root,
  );
}
