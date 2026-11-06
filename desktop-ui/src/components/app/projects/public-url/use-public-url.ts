import { useQuery } from "@tanstack/react-query";

import { bootstrapQueries, cloudQueries } from "@/lib/queries";
import { findPublicUrl, projectNamespaceDomain, type PublicUrlMatch } from "@/lib/util.cloud";

/**
 * The public URL that serves a local domain, when signed in to Localify Cloud: its own, or
 * its project's URL, which serves every domain under the project's namespace.
 */
export function usePublicUrl(localDomain: string, projectId: string | null): PublicUrlMatch | null {
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const { data: account } = useQuery(cloudQueries.account());
  const signedIn = account?.signedIn ?? false;
  const { data: publicUrls } = useQuery({ ...cloudQueries.publicUrls(), enabled: signedIn });
  if (!signedIn) return null;
  const namespace =
    projectId === null
      ? null
      : projectNamespaceDomain(projectId, bootstrap?.environment.fixedDomainSuffix ?? "");
  return findPublicUrl(publicUrls, localDomain, namespace);
}
