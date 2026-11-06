<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { BOOKMARKS_BAR, type DataBookmark } from '../lib/api/types-data';
  import Favicon from '../lib/components/Favicon.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { editBookmark, renameFolder } from '../lib/data/edit';
  import { onDataChanged, onReconnect } from '../lib/data/library';
  import {
    canDropInto,
    dropPosition,
    findFolder,
    flattenFolders,
    folderPath,
    loadTree,
    type FolderNode,
    type TreeNode,
  } from '../lib/data/tree';
  import { isSafeLinkUrl } from '../lib/safeurl';
  import { hostOf } from '../lib/urls';
  import Dialog from './Dialog.svelte';
  import FolderTree from './FolderTree.svelte';

  let {
    api,
    folder = $bindable(BOOKMARKS_BAR),
    notify,
    fail,
  }: {
    api: BrowserApi;
    folder?: number;
    notify: (text: string) => void;
    fail: (err: unknown) => void;
  } = $props();

  let tree = $state<FolderNode[]>([]);
  let loaded = $state(false);
  let expanded = $state<Record<number, boolean>>({});
  let query = $state('');
  let results = $state<DataBookmark[] | null>(null);
  let searchTick = $state(0);

  const current = $derived(findFolder(tree, folder) ?? tree[0] ?? null);
  const path = $derived(current ? folderPath(tree, current.id) : []);
  const folderTitles = $derived(new Map(flattenFolders(tree).map(({ node }) => [node.id, node.title])));

  async function reload() {
    try {
      tree = await loadTree(api);
      loaded = true;
      if (!findFolder(tree, folder)) folder = BOOKMARKS_BAR;
      for (const p of folderPath(tree, folder)) expanded[p.id] = true;
    } catch (err) {
      fail(err);
    }
  }
  $effect(() => {
    void reload();
    // Edits by anyone (the star, an agent, another library tab) arrive as bookmarks.changed.
    const offChanged = onDataChanged(api, 'bookmarks.changed', () => {
      void reload();
      searchTick++;
    });
    const offReconnect = onReconnect(api, () => void reload());
    return () => {
      offChanged();
      offReconnect();
    };
  });

  // Search across all folders (debounced).
  $effect(() => {
    const q = query.trim();
    void searchTick;
    if (!q) {
      results = null;
      return;
    }
    const t = setTimeout(() => {
      api
        .call('bookmarks.search', { q, limit: 200 })
        .then((r) => q === query.trim() && (results = r.bookmarks))
        .catch(fail);
    }, 180);
    return () => clearTimeout(t);
  });

  async function mutate(work: () => Promise<unknown>, done?: string) {
    try {
      await work();
      if (done) notify(done);
    } catch (err) {
      fail(err);
    }
    await reload();
    searchTick++; // re-run the search
  }

  // ---- drag & drop ------------------------------------------------------------------------

  let dragged = $state<number | null>(null);
  let dropFolder = $state<number | null>(null);
  let dropRow = $state<{ id: number; where: 'before' | 'after' | 'into' } | null>(null);

  function onDragStart(e: DragEvent, id: number) {
    dragged = id;
    e.dataTransfer?.setData('application/x-bmcp-bookmark', String(id));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }
  function onDragEnd() {
    dragged = null;
    dropFolder = null;
    dropRow = null;
  }
  function overFolder(e: DragEvent, id: number) {
    if (dragged === null || !canDropInto(tree, dragged, id)) return;
    e.preventDefault();
    dropFolder = id;
  }
  function dropOnFolder(e: DragEvent, id: number) {
    e.preventDefault();
    const d = dragged;
    onDragEnd();
    if (d === null || !canDropInto(tree, d, id)) return;
    void mutate(() => api.call('bookmarks.move', { id: d, folder: id }));
  }
  function overRow(e: DragEvent, node: TreeNode) {
    if (dragged === null || dragged === node.id) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const y = (e.clientY - r.top) / r.height;
    let where: 'before' | 'after' | 'into' = y < 0.5 ? 'before' : 'after';
    if (node.kind === 'folder' && y > 0.25 && y < 0.75) {
      if (!canDropInto(tree, dragged, node.id)) return;
      where = 'into';
    }
    e.preventDefault();
    dropRow = { id: node.id, where };
  }
  function dropOnRow(e: DragEvent) {
    e.preventDefault();
    const d = dragged;
    const target = dropRow;
    onDragEnd();
    if (d === null || !target || !current) return;
    if (target.where === 'into') {
      void mutate(() => api.call('bookmarks.move', { id: d, folder: target.id }));
      return;
    }
    if (!canDropInto(tree, d, current.id)) return;
    const siblings = current.children.map((c) => c.id);
    const position = dropPosition(siblings, d, target.id, target.where === 'after');
    void mutate(() => api.call('bookmarks.move', { id: d, folder: current.id, position }));
  }

  // ---- dialogs ------------------------------------------------------------------------------

  type Editing =
    | { mode: 'add-url'; title: string; url: string; folder: number }
    | { mode: 'add-folder'; title: string; folder: number }
    | { mode: 'edit-url'; node: { id: number; folder: number; position: number }; title: string; url: string }
    | { mode: 'edit-folder'; node: FolderNode; title: string };
  let editing = $state<Editing | null>(null);
  let dialogOpen = $state(false);
  let editError = $state<string | null>(null);
  let confirmRemove = $state<FolderNode | null>(null);
  let confirmOpen = $state(false);

  function open(e: Editing) {
    editing = e;
    editError = null;
    dialogOpen = true;
  }
  $effect(() => {
    if (!dialogOpen) editing = null;
  });

  /** A full http(s)/file URL, or `example.com` made into https; anything else (javascript:,
   * data:, …) is refused here and by the core. */
  function validUrl(u: string): string | null {
    const allowed = (href: string) => (['http:', 'https:', 'file:'].includes(new URL(href).protocol) ? href : null);
    try {
      return allowed(new URL(u.trim()).href);
    } catch {
      try {
        return /^[\w-]+(\.[\w-]+)+/.test(u.trim()) ? allowed(new URL(`https://${u.trim()}`).href) : null;
      } catch {
        return null;
      }
    }
  }

  async function submit() {
    const e = editing;
    if (!e) return;
    const title = e.title.trim();
    let url: string | null = null;
    if (e.mode === 'add-url' || e.mode === 'edit-url') {
      url = validUrl(e.url);
      if (!url) return void (editError = 'Enter a full address, e.g. https://example.com/');
    } else if (!title) {
      return void (editError = 'A folder needs a name.');
    }
    dialogOpen = false;
    switch (e.mode) {
      case 'add-url':
        return mutate(() => api.call('bookmarks.add', { folder: e.folder, title: title || null, url: url! }), 'Bookmark added');
      case 'add-folder':
        return mutate(async () => {
          const f = await api.call('bookmarks.add_folder', { parent: e.folder, title });
          expanded[e.folder] = true;
          return f;
        }, 'Folder added');
      case 'edit-url':
        return mutate(() => editBookmark(api, e.node.id, title || url!, url!), 'Bookmark saved');
      case 'edit-folder': {
        if (title === e.node.title) return;
        return mutate(() => renameFolder(api, e.node.id, title), 'Folder renamed');
      }
    }
  }

  function edit(node: TreeNode) {
    if (node.kind === 'url') open({ mode: 'edit-url', node: { id: node.id, folder: node.parent, position: node.position }, title: node.title, url: node.url });
    else open({ mode: 'edit-folder', node, title: node.title });
  }
  function editFound(b: DataBookmark) {
    open({ mode: 'edit-url', node: { id: b.id, folder: b.folder, position: b.position }, title: b.title, url: b.url });
  }

  function remove(node: TreeNode | DataBookmark) {
    if ('kind' in node && node.kind === 'folder') {
      if (node.children.length) {
        confirmRemove = node;
        confirmOpen = true;
        return;
      }
    }
    const name = node.title || ('url' in node ? node.url : 'folder');
    void mutate(() => api.call('bookmarks.remove', { id: node.id }), `Removed “${name}”`);
  }
  function confirmRemoveFolder() {
    const n = confirmRemove;
    confirmOpen = false;
    if (!n) return;
    if (folder === n.id) folder = n.parent ?? BOOKMARKS_BAR;
    void mutate(() => api.call('bookmarks.remove', { id: n.id }), `Removed folder “${n.title}”`);
  }

  const countLabel = (n: FolderNode) => {
    const folders = n.children.filter((c) => c.kind === 'folder').length;
    const urls = n.children.length - folders;
    return [urls && `${urls} bookmark${urls === 1 ? '' : 's'}`, folders && `${folders} folder${folders === 1 ? '' : 's'}`]
      .filter(Boolean)
      .join(', ') || 'Empty';
  };
</script>

<header class="head">
  <h2>Bookmarks</h2>
  <label class="lib-search">
    <Icon name="search" size={14} />
    <input class="lib-input" type="search" placeholder="Search bookmarks" aria-label="Search bookmarks" bind:value={query} />
  </label>
  <div class="spacer"></div>
  <button class="btn" onclick={() => open({ mode: 'add-folder', title: '', folder: current?.id ?? BOOKMARKS_BAR })}>
    <Icon name="folder-plus" size={14} /> Add folder
  </button>
  <button class="btn primary" onclick={() => open({ mode: 'add-url', title: '', url: '', folder: current?.id ?? BOOKMARKS_BAR })}>
    <Icon name="plus" size={14} stroke={2.25} /> Add bookmark
  </button>
</header>

<div class="cols">
  <aside>
    <FolderTree
      nodes={tree}
      selected={current?.id ?? BOOKMARKS_BAR}
      {expanded}
      dropTarget={dropFolder}
      onselect={(id) => {
        folder = id;
        query = '';
      }}
      ontoggle={(id) => (expanded[id] = !(expanded[id] ?? tree.some((r) => r.id === id)))}
      ondragstart={onDragStart}
      ondragover={overFolder}
      ondrop={dropOnFolder}
      ondragend={onDragEnd}
    />
  </aside>

  <section class="list" aria-label="Bookmarks">
    {#if results}
      <p class="crumbs">{results.length} result{results.length === 1 ? '' : 's'} for “{query.trim()}”</p>
      {#each results as b (b.id)}
        <div class="lib-row">
          <Favicon url={b.url} size={16} />
          <div class="grow">
            {#if isSafeLinkUrl(b.url)}
              <a class="title" href={b.url}>{b.title || b.url}</a>
            {:else}
              <span class="title">{b.title || b.url}</span>
            {/if}
            <span class="sub">{hostOf(b.url) || b.url} · in {folderTitles.get(b.folder) ?? 'a folder'}</span>
          </div>
          <div class="actions">
            <button class="icon-btn" aria-label="Show in folder" title="Show in folder" onclick={() => ((folder = b.folder), (query = ''))}>
              <Icon name="folder" size={15} />
            </button>
            <button class="icon-btn" aria-label="Edit {b.title}" title="Edit" onclick={() => editFound(b)}><Icon name="pencil" size={15} /></button>
            <button class="icon-btn" aria-label="Delete {b.title}" title="Delete" onclick={() => remove(b)}><Icon name="trash" size={15} /></button>
          </div>
        </div>
      {:else}
        <div class="lib-empty"><h3>No bookmarks match</h3></div>
      {/each}
    {:else if current}
      <p class="crumbs">
        {#each path as p, i (p.id)}
          {#if i > 0}<Icon name="chevron-right" size={12} />{/if}
          <button class="crumb" onclick={() => (folder = p.id)} aria-current={p.id === current.id ? 'location' : undefined}>{p.title}</button>
        {/each}
        <span class="count">{countLabel(current)}</span>
      </p>
      <ul class="rows" role="list" ondragleave={(e) => e.currentTarget === e.target && (dropRow = null)}>
        {#each current.children as node (node.id)}
          <li
            class="lib-row"
            class:dragging={dragged === node.id}
            class:drop-before={dropRow?.id === node.id && dropRow.where === 'before'}
            class:drop-after={dropRow?.id === node.id && dropRow.where === 'after'}
            class:drop-into={dropRow?.id === node.id && dropRow.where === 'into'}
            draggable="true"
            ondragstart={(e) => onDragStart(e, node.id)}
            ondragover={(e) => overRow(e, node)}
            ondrop={dropOnRow}
            ondragend={onDragEnd}
            ondblclick={() => node.kind === 'folder' && (folder = node.id)}
          >
            {#if node.kind === 'folder'}
              <span class="ficon"><Icon name="folder" size={16} /></span>
              <div class="grow">
                <button class="title as-link" onclick={() => (folder = node.id)}>{node.title}</button>
                <span class="sub">{countLabel(node)}</span>
              </div>
            {:else}
              <Favicon url={node.url} size={16} />
              <div class="grow">
                {#if isSafeLinkUrl(node.url)}
                  <a class="title" href={node.url} draggable="false">{node.title || node.url}</a>
                {:else}
                  <span class="title" title="Only http(s) addresses open from here">{node.title || node.url}</span>
                {/if}
                <span class="sub">{node.url}</span>
              </div>
            {/if}
            <div class="actions">
              <button class="icon-btn" aria-label="Edit {node.title}" title="Edit" onclick={() => edit(node)}><Icon name="pencil" size={15} /></button>
              <button class="icon-btn" aria-label="Delete {node.title}" title="Delete" onclick={() => remove(node)}><Icon name="trash" size={15} /></button>
            </div>
          </li>
        {:else}
          <li class="lib-empty">
            <h3>This folder is empty</h3>
            <p>Add a bookmark here, star a page in the address bar, or drag bookmarks in from another folder.</p>
          </li>
        {/each}
      </ul>
    {:else if loaded}
      <div class="lib-empty"><h3>No bookmark folders</h3></div>
    {/if}
  </section>
</div>

<Dialog
  bind:open={dialogOpen}
  title={editing?.mode === 'add-url' ? 'Add bookmark' : editing?.mode === 'add-folder' ? 'Add folder' : editing?.mode === 'edit-folder' ? 'Rename folder' : 'Edit bookmark'}
  onsubmit={submit}
>
  {#if editing}
    <label class="lib-field">
      {editing.mode === 'add-folder' || editing.mode === 'edit-folder' ? 'Folder name' : 'Name'}
      <!-- svelte-ignore a11y_autofocus -->
      <input class="lib-input" bind:value={editing.title} autofocus />
    </label>
    {#if editing.mode === 'add-url' || editing.mode === 'edit-url'}
      <label class="lib-field">
        Address
        <input class="lib-input mono" bind:value={editing.url} placeholder="https://" spellcheck="false" />
      </label>
    {/if}
    {#if editing.mode === 'add-url' || editing.mode === 'add-folder'}
      <label class="lib-field">
        In folder
        <select class="lib-input" bind:value={editing.folder}>
          {#each flattenFolders(tree) as { node, depth } (node.id)}
            <option value={node.id}>{'   '.repeat(depth)}{node.title}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if editError}<p class="lib-error">{editError}</p>{/if}
  {/if}
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (dialogOpen = false)}>Cancel</button>
    <button type="submit" class="btn primary">Save</button>
  {/snippet}
</Dialog>

<Dialog bind:open={confirmOpen} title="Delete folder?" onsubmit={confirmRemoveFolder}>
  <p>“{confirmRemove?.title}” and everything inside it ({confirmRemove ? countLabel(confirmRemove) : ''}) will be deleted.</p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (confirmOpen = false)}>Cancel</button>
    <button type="submit" class="btn primary danger">Delete</button>
  {/snippet}
</Dialog>

<style>
  .head {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 24px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0 8px 0 0;
    font-size: 18px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .cols {
    display: grid;
    grid-template-columns: 250px minmax(0, 1fr);
    min-height: calc(100% - 60px);
  }
  aside {
    padding: 10px 8px;
    border-right: 1px solid var(--border);
  }
  .list {
    padding: 8px 18px 40px;
  }
  .crumbs {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 2px;
    margin: 4px 0 8px;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .crumb {
    padding: 2px 4px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--fg-muted);
    font-weight: 550;
  }
  .crumb:hover {
    background: var(--hover);
  }
  .crumb[aria-current] {
    color: var(--fg);
  }
  .count {
    margin-left: 10px;
  }
  .rows {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .ficon {
    display: inline-grid;
    width: 16px;
    color: var(--fg-subtle);
  }
  .as-link {
    padding: 0;
    border: 0;
    background: transparent;
    font-weight: 550;
    text-align: left;
  }
  .as-link:hover {
    text-decoration: underline;
  }
  .lib-row {
    position: relative;
  }
  .lib-row.dragging {
    opacity: 0.45;
  }
  .lib-row.drop-before::before,
  .lib-row.drop-after::after {
    content: '';
    position: absolute;
    left: 6px;
    right: 6px;
    height: 2px;
    border-radius: 1px;
    background: var(--accent);
  }
  .lib-row.drop-before::before {
    top: -1px;
  }
  .lib-row.drop-after::after {
    bottom: -1px;
  }
  .lib-row.drop-into {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  select.lib-input {
    padding-right: 4px;
  }
  @media (max-width: 720px) {
    .head {
      flex-wrap: wrap;
    }
    .cols {
      grid-template-columns: minmax(0, 1fr);
    }
    aside {
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
  }
</style>
