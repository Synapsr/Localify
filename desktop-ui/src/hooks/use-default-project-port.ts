import { useMutation } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import { utilityMutations } from "@/lib/queries";

interface UseDefaultProjectPortOptions {
  enabled: boolean;
  getTargetValue: () => string;
  setTargetValue: (value: string) => void;
  setError: (value: string | null) => void;
}

export const useDefaultProjectPort = ({
  enabled,
  getTargetValue,
  setTargetValue,
  setError,
}: UseDefaultProjectPortOptions) => {
  const availablePort = useMutation(utilityMutations.availablePort());

  const getTargetValueRef = useRef(getTargetValue);
  const setTargetValueRef = useRef(setTargetValue);
  const setErrorRef = useRef(setError);

  getTargetValueRef.current = getTargetValue;
  setTargetValueRef.current = setTargetValue;
  setErrorRef.current = setError;

  useEffect(() => {
    if (!enabled || getTargetValueRef.current().trim().length > 0) {
      return;
    }

    let cancelled = false;

    availablePort
      .mutateAsync()
      .then(({ port }) => {
        if (cancelled) {
          return;
        }
        if (getTargetValueRef.current().trim().length > 0) {
          return;
        }
        setTargetValueRef.current(String(port));
      })
      .catch(() => {
        // Ignore — user can still press "Free port" manually.
      });

    return () => {
      cancelled = true;
    };
  }, [availablePort, enabled]);

  const generatePort = async () => {
    setErrorRef.current(null);

    try {
      const { port } = await availablePort.mutateAsync();
      setTargetValueRef.current(String(port));
    } catch (rawError) {
      setErrorRef.current(rawError instanceof Error ? rawError.message : String(rawError));
    }
  };

  return {
    generatePort,
    isPending: availablePort.isPending,
  };
};
