import { useEffect, useState, type DependencyList } from "react";
import { errorText } from "./api";

export function useResource<T>(load: () => Promise<T>, dependencies: DependencyList) {
  const [data, setData] = useState<T>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let current = true;
    load().then(
      value => {
        if (!current) return;
        setData(value);
        setError(undefined);
      },
      error => {
        if (current) setError(errorText(error));
      },
    );
    return () => { current = false; };
    // Callers supply the dependencies of the load operation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, dependencies);
  return { data, error, setData };
}
