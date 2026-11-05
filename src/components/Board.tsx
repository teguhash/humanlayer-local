import { useState } from "react";
import { STAGES, STAGE_SKILL, type Stage, type Task } from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * Kanban board. Drag a card between or within columns; `onMove` gets the
 * target stage and index (0 = top). Native HTML5 drag and drop.
 */
export function Board(props: {
  tasks: Task[];
  onMove: (task: Task, stage: Stage, index: number) => void;
  onOpen: (id: number) => void;
  label?: (t: Task) => string;
}) {
  const [dragId, setDragId] = useState<number | null>(null);
  const [drop, setDrop] = useState<{ stage: Stage; index: number } | null>(null);

  const columns = STAGES.map((stage) => ({
    stage,
    tasks: props.tasks.filter((t) => t.stage === stage).sort((a, b) => a.position - b.position || a.id - b.id),
  }));

  // Index among the column's cards (not counting the dragged one) where the
  // pointer is: before the first card whose middle is below it.
  function indexAt(e: React.DragEvent, col: HTMLElement): number {
    const cards = [...col.querySelectorAll<HTMLElement>("[data-card]")].filter(
      (c) => Number(c.dataset.card) !== dragId,
    );
    const i = cards.findIndex((c) => {
      const r = c.getBoundingClientRect();
      return e.clientY < r.top + r.height / 2;
    });
    return i === -1 ? cards.length : i;
  }

  return (
    <div className="flex min-h-0 flex-1 gap-2 overflow-x-auto pb-2">
      {columns.map(({ stage, tasks }) => (
        <section
          key={stage}
          className={cn(
            "flex w-52 flex-none flex-col border bg-secondary/20",
            drop?.stage === stage && "border-accent",
          )}
          onDragOver={(e) => {
            if (dragId === null) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            const index = indexAt(e, e.currentTarget);
            if (drop?.stage !== stage || drop.index !== index) setDrop({ stage, index });
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node)) setDrop(null);
          }}
          onDrop={(e) => {
            e.preventDefault();
            const task = props.tasks.find((t) => t.id === dragId);
            const index = indexAt(e, e.currentTarget);
            setDrop(null);
            setDragId(null);
            if (task) props.onMove(task, stage, index);
          }}
        >
          <header className="border-b px-2 py-1.5">
            <div className="flex items-baseline justify-between">
              <span className="font-semibold capitalize">{stage}</span>
              <span className="text-xs text-muted-foreground">{tasks.length}</span>
            </div>
            <div className="truncate text-xs text-muted-foreground">
              {STAGE_SKILL[stage] ? `starts /rpi:${STAGE_SKILL[stage]}` : "\u00a0"}
            </div>
          </header>
          <div className="flex min-h-16 flex-1 flex-col gap-1.5 overflow-y-auto p-1.5">
            {tasks.map((t, i) => (
              <div key={t.id}>
                {drop?.stage === stage && drop.index === i && dragId !== t.id && <DropLine />}
                <article
                  data-card={t.id}
                  draggable
                  onDragStart={(e) => {
                    setDragId(t.id);
                    e.dataTransfer.effectAllowed = "move";
                    e.dataTransfer.setData("text/plain", String(t.id));
                  }}
                  onDragEnd={() => {
                    setDragId(null);
                    setDrop(null);
                  }}
                  onClick={() => props.onOpen(t.id)}
                  className={cn(
                    "cursor-grab border bg-background p-2 hover:border-accent active:cursor-grabbing",
                    dragId === t.id && "opacity-40",
                  )}
                >
                  <div className="line-clamp-3">{t.title}</div>
                  {props.label?.(t) && <div className="mt-1 text-xs text-muted-foreground">{props.label(t)}</div>}
                </article>
              </div>
            ))}
            {drop?.stage === stage && drop.index >= tasks.filter((t) => t.id !== dragId).length && <DropLine />}
          </div>
        </section>
      ))}
    </div>
  );
}

function DropLine() {
  return <div className="my-0.5 h-0.5 bg-accent" />;
}
