import { Program, type Idl } from "@coral-xyz/anchor";
import type { Connection } from "@solana/web3.js";

/**
 * Builds an Anchor client for coded_router from the IDL that `anchor build`
 * writes to target/idl/coded_router.json. Only used to build instructions,
 * so a bare `{ connection }` provider is enough.
 */
export function codedProgram(connection: Connection, idl: Idl): Program {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return new Program(idl, { connection } as any);
}

export interface RouteConfigArgs {
  holdersBps: number;
  lpBps: number;
  burnBps: number;
  minHolderBalance: bigint;
  maxSlippageBps: number;
  lpMode: number;
  claimIntervalSecs: number;
  excludeCreator: boolean;
}
