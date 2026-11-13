/** Minimal typed event emitter. Listener errors are isolated so one bad listener cannot stop capture. */
export class Emitter<Events extends object> {
  private readonly listeners = new Map<keyof Events, Set<(value: never) => void>>();

  on<K extends keyof Events>(type: K, listener: (value: Events[K]) => void): () => void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener);
    return () => {
      this.listeners.get(type)?.delete(listener);
    };
  }

  emit<K extends keyof Events>(type: K, value: Events[K]): void {
    for (const listener of [...(this.listeners.get(type) ?? [])]) {
      try {
        (listener as (value: Events[K]) => void)(value);
      } catch (error) {
        queueMicrotask(() => {
          throw error;
        });
      }
    }
  }

  clear(): void {
    this.listeners.clear();
  }
}
