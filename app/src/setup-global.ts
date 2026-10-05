/**
 * One-time protocol setup: creates the Global account and loads the CPI
 * allowlist with discriminators read straight from pump.fun's own IDLs.
 *
 *   ADMIN_KEYPAIR=~/.config/solana/id.json ROOT_POSTER=<pubkey> npx tsx src/setup-global.ts
 */
import { readFileSync } from "node:fs";
import { AnchorProvider, Program, Wallet, type Idl } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { PUMP_AMM_PROGRAM_ID, PUMP_PROGRAM_ID, pumpIdl } from "@pump-fun/pump-sdk";
import { getPumpAmmProgram } from "@pump-fun/pump-swap-sdk";

import { CPI_KIND_DEPOSIT, CPI_KIND_SWAP, globalPda } from "./constants";

const RPC = process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com";
const IDL_PATH = process.env.IDL_PATH ?? "../target/idl/coded_router.json";

type IxIdl = { name: string; discriminator: number[] };

function disc(idl: { instructions: IxIdl[] }, name: string): number[] | null {
  const ix = idl.instructions.find((i) => i.name === name);
  return ix ? ix.discriminator : null;
}

async function main() {
  const connection = new Connection(RPC, "confirmed");
  const admin = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(readFileSync(process.env.ADMIN_KEYPAIR!, "utf8"))),
  );
  const rootPoster = new PublicKey(process.env.ROOT_POSTER ?? admin.publicKey.toBase58());
  const idl = JSON.parse(readFileSync(IDL_PATH, "utf8")) as Idl;
  const program = new Program(idl, new AnchorProvider(connection, new Wallet(admin), {}));

  const pump = pumpIdl as unknown as { instructions: IxIdl[] };
  const amm = getPumpAmmProgram(connection).idl as unknown as { instructions: IxIdl[] };

  const wanted: [PublicKey, { instructions: IxIdl[] }, string, number][] = [
    [PUMP_PROGRAM_ID, pump, "buy", CPI_KIND_SWAP],
    [PUMP_PROGRAM_ID, pump, "buy_v2", CPI_KIND_SWAP],
    [PUMP_PROGRAM_ID, pump, "buy_exact_sol_in", CPI_KIND_SWAP],
    [PUMP_PROGRAM_ID, pump, "buy_exact_quote_in_v2", CPI_KIND_SWAP],
    [PUMP_AMM_PROGRAM_ID, amm, "buy", CPI_KIND_SWAP],
    [PUMP_AMM_PROGRAM_ID, amm, "buy_exact_quote_in", CPI_KIND_SWAP],
    [PUMP_AMM_PROGRAM_ID, amm, "deposit", CPI_KIND_DEPOSIT],
  ];
  const allowed = wanted.flatMap(([programId, i, name, kind]) => {
    const d = disc(i, name);
    if (!d) {
      console.warn(`skipping ${name}: not in this IDL version`);
      return [];
    }
    return [{ programId, discriminator: d, kind }];
  });

  const global = globalPda();
  if (!(await connection.getAccountInfo(global))) {
    await program.methods
      .initGlobal(rootPoster, 10, 200) // 0.1% crank tip, 2% fee allowance
      .accountsPartial({ admin: admin.publicKey, global, systemProgram: SystemProgram.programId })
      .rpc();
    console.log("global created", global.toBase58());
  }
  await program.methods.setAllowlist(allowed).accountsPartial({ admin: admin.publicKey, global }).rpc();
  console.log(`allowlist set with ${allowed.length} entries`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
