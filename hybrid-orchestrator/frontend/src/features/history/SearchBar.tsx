// SearchBar (architecture.md Section 8.5 history surface).
//
// A purely client-side filter input over the conversation index: it owns no
// store state and simply reports the current query text to its parent, which
// filters the list by title and tags.

export interface SearchBarProps {
  /** The current query text. */
  value: string;
  /** Called with the new query whenever the input changes. */
  onChange: (query: string) => void;
}

export function SearchBar({ value, onChange }: SearchBarProps) {
  return (
    <div className="search-bar">
      <input
        type="search"
        className="search-bar__input"
        aria-label="Search conversations"
        placeholder="Search conversations"
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}
