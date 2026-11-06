import { useQuery } from "@tanstack/react-query";

import type { PublicUrlDto } from "@/types/localify";

import { cloudQueries } from "@/lib/queries";
import { findPublicUrl } from "@/lib/util.cloud";

/** The public URL of a local domain, when signed in to Localify Cloud and it has one. */
export function usePublicUrl(localDomain: string): PublicUrlDto | null {
  const { data: account } = useQuery(cloudQueries.account());
  const signedIn = account?.signedIn ?? false;
  const { data: publicUrls } = useQuery({ ...cloudQueries.publicUrls(), enabled: signedIn });
  return signedIn ? findPublicUrl(publicUrls, localDomain) : null;
}
