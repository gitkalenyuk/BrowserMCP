<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import type { FolderNode } from '../lib/data/tree';
  import FolderTree from './FolderTree.svelte';

  // The folder column of the bookmarks section. Folders are drop targets ("move into") and,
  // except the two roots, draggable themselves.
  let {
    nodes,
    selected,
    expanded,
    depth = 0,
    dropTarget,
    onselect,
    ontoggle,
    ondragstart,
    ondragover,
    ondrop,
    ondragend,
  }: {
    nodes: FolderNode[];
    selected: number;
    expanded: Record<number, boolean>;
    depth?: number;
    dropTarget: number | null;
    onselect: (id: number) => void;
    ontoggle: (id: number) => void;
    ondragstart: (e: DragEvent, id: number) => void;
    ondragover: (e: DragEvent, id: number) => void;
    ondrop: (e: DragEvent, id: number) => void;
    ondragend: () => void;
  } = $props();

  const subfolders = (n: FolderNode) => n.children.filter((c): c is FolderNode => c.kind === 'folder');
</script>

<ul role={depth === 0 ? 'tree' : 'group'} aria-label={depth === 0 ? 'Bookmark folders' : undefined}>
  {#each nodes as n (n.id)}
    {@const subs = subfolders(n)}
    {@const open = expanded[n.id] ?? depth === 0}
    <li role="treeitem" aria-selected={selected === n.id} aria-expanded={subs.length ? open : undefined}>
      <div
        class="item"
        class:selected={selected === n.id}
        class:drop={dropTarget === n.id}
        style:padding-left="{6 + depth * 14}px"
        draggable={n.parent !== null}
        role="presentation"
        ondragstart={(e) => ondragstart(e, n.id)}
        ondragover={(e) => ondragover(e, n.id)}
        ondrop={(e) => ondrop(e, n.id)}
        ondragend={ondragend}
      >
        <button
          class="twisty"
          class:hidden={!subs.length}
          tabindex="-1"
          aria-label={open ? 'Collapse' : 'Expand'}
          onclick={() => ontoggle(n.id)}
        >
          <Icon name={open ? 'chevron-down' : 'chevron-right'} size={13} stroke={2} />
        </button>
        <button class="name" onclick={() => onselect(n.id)} title={n.title}>
          <Icon name="folder" size={15} />
          <span>{n.title}</span>
        </button>
      </div>
      {#if subs.length && open}
        <FolderTree
          nodes={subs}
          {selected}
          {expanded}
          depth={depth + 1}
          {dropTarget}
          {onselect}
          {ontoggle}
          {ondragstart}
          {ondragover}
          {ondrop}
          {ondragend}
        />
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 30px;
    padding-right: 6px;
    border-radius: var(--r);
  }
  .item:hover {
    background: var(--hover);
  }
  .item.selected {
    background: var(--accent-soft);
  }
  .item.selected .name {
    color: var(--accent);
  }
  .item.drop {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .twisty {
    display: inline-grid;
    place-items: center;
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--fg-subtle);
    flex: none;
  }
  .twisty.hidden {
    visibility: hidden;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 4px;
    border: 0;
    background: transparent;
    color: var(--fg-muted);
    font-weight: 550;
    text-align: left;
  }
  .name span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
