# LumenLiquid — Frontend Trading Integration Guide

How a web frontend calls the **PositionManager** Soroban contract to open
market trades, place limit orders, and manage TP/SL. Covers wallet auth, USDC
approval, value scaling, and error handling.

> Audience: frontend devs using `@stellar/stellar-sdk` + a Soroban wallet
> (Freighter, Lobstr, xBull, or any wallet exposing the standard signing API).

---

## Contract IDs (testnet)

| Contract          | ID                                                         | Role |
|-------------------|------------------------------------------------------------|------|
| PositionManager   | `CCDJJE7IUHFANGMB76BDD5DKWWAUZFUTENIKLRJLYQAUNWLSM7NTU72U`  | open/close/limit/tp-sl |
| PairRegistry      | `CCENPBAIKAYGL6A2IVYZTOPJUQCPLXZANEMISD4SB7XFWVRJA6PQPJVW`  | pair config, OI |
| Vault             | `CALSWR6YPUQBMP3JE4SJGNHGAWT756DYNPCSMPE4CM27ORAF4BU3Y4AW`  | LP deposits, PnL settlement |

Network: **Testnet** — RPC `https://soroban-testnet.stellar.org`,
passphrase `Test SDF Network ; September 2015`.

USDC is the collateral token (SEP-41). Get its address from the Vault:
`vault.usdc_token()`.

---

## Value scaling — read this first

All on-chain integers are scaled. Convert at the UI boundary; never pass floats
to the contract.

| Quantity         | Scale       | Example |
|------------------|-------------|---------|
| USDC / collateral / PnL / fees | `1e7` (USDC_SCALE) | 100 USDC → `1000000000` |
| Prices (open/tp/sl/limit/liq)  | `1e10` (PRICE_SCALE) | $65,000.50 → `650005000000` |
| Leverage          | integer (no scale) | 10x → `10` |
| Percent rates (fees, thresholds) | `1e10` (P_SCALE) | 0.08% → `8000000` |

```ts
const USDC_SCALE  = 10_000_000n;        // 1e7
const PRICE_SCALE = 10_000_000_000n;    // 1e10

const toUsdc  = (h: number) => BigInt(Math.round(h * 1e7));      // human → on-chain
const fromUsdc = (r: bigint) => Number(r) / 1e7;                  // on-chain → human
const toPrice  = (h: number) => BigInt(Math.round(h * 1e10));
const fromPrice = (r: bigint) => Number(r) / 1e10;
```

> Prices come out of the Reflector oracle and are normalized to `1e10`. TP/SL/
> limit prices you submit MUST be at `1e10` too.

---

## Pair indices

| Index | Symbol  |
|-------|---------|
| 0 | BTC/USD |
| 1 | ETH/USD |
| 2 | SOL/USD |
| 3 | BNB/USD |

Read live pair config (leverage range, fees, liq threshold, disabled flag) from
the registry — do not hardcode. See [Reading pair config](#reading-pair-config).

---

## Setup

```bash
npm i @stellar/stellar-sdk
```

```ts
import {
  Contract, TransactionBuilder, BASE_FEE, Networks,
  rpc, scValToNative, nativeToScVal, Address,
} from "@stellar/stellar-sdk";

const server = new rpc.Server("https://soroban-testnet.stellar.org");
const NETWORK = Networks.TESTNET;

const PM_ID       = "CCDJJE7IUHFANGMB76BDD5DKWWAUZFUTENIKLRJLYQAUNWLSM7NTU72U";
const REGISTRY_ID = "CCENPBAIKAYGL6A2IVYZTOPJUQCPLXZANEMISD4SB7XFWVRJA6PQPJVW";
const VAULT_ID    = "CALSWR6YPUQBMP3JE4SJGNHGAWT756DYNPCSMPE4CM27ORAF4BU3Y4AW";
```

### scval helpers

Argument types must match the contract ABI exactly or the host rejects the call.

```ts
const u32  = (n: number) => nativeToScVal(n, { type: "u32" });
const i128 = (n: bigint) => nativeToScVal(n, { type: "i128" });
const bool = (b: boolean) => nativeToScVal(b, { type: "bool" });
const addr = (a: string)  => new Address(a).toScVal();
```

### The invoke flow (build → simulate → sign → send → poll)

Every state-changing call follows the same pipeline. The wallet handles
`require_auth()` by signing the auth entries during `signTransaction`.

```ts
async function invoke(
  contractId: string,
  method: string,
  args: any[],
  walletAddress: string,
  signTransaction: (xdr: string, opts: any) => Promise<{ signedTxXdr: string }>,
) {
  const account = await server.getAccount(walletAddress);
  const contract = new Contract(contractId);

  let tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: NETWORK })
    .addOperation(contract.call(method, ...args))
    .setTimeout(60)
    .build();

  // 1. Simulate → get footprint, auth, resource fees
  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) {
    throw new Error(parseContractError(sim.error)); // see Error Handling
  }

  // 2. Prepare (assembles the simulated footprint + soroban auth)
  tx = rpc.assembleTransaction(tx, sim).build();

  // 3. Sign with the wallet
  const { signedTxXdr } = await signTransaction(tx.toXDR(), {
    networkPassphrase: NETWORK,
    address: walletAddress,
  });
  const signed = TransactionBuilder.fromXDR(signedTxXdr, NETWORK);

  // 4. Send + poll
  const sent = await server.sendTransaction(signed);
  if (sent.status === "ERROR") throw new Error("send failed: " + JSON.stringify(sent.errorResult));

  let res = await server.getTransaction(sent.hash);
  while (res.status === "NOT_FOUND") {
    await new Promise(r => setTimeout(r, 1000));
    res = await server.getTransaction(sent.hash);
  }
  if (res.status !== "SUCCESS") throw new Error("tx failed: " + res.status);

  return { hash: sent.hash, returnValue: res.returnValue && scValToNative(res.returnValue) };
}
```

> Read-only views (`get_trade`, `get_pair`, `get_trade_pnl`) only need the
> simulate step — never sign or send them. See [Reading state](#reading-state).

---

## Step 1 — Approve USDC (one-time per allowance)

`open_market_trade` and `place_limit_order` pull collateral via
`usdc.transfer(trader, PM, collateral)`, which the contract calls on the
trader's behalf. The transfer auth is bundled into the trade transaction's auth
tree, so the wallet signs it together with the trade — **no separate approve tx
is required** for the direct-transfer path.

What you DO need: the trader must hold enough USDC, and the wallet must sign the
sub-invocation. If your wallet flow needs an explicit SEP-41 `approve` (some
integrations prefer allowance-based spending), call:

```ts
// optional: explicit allowance, expires at a future ledger
await invoke(USDC_ID, "approve", [
  addr(trader), addr(PM_ID), i128(toUsdc(1000)), u32(currentLedger + 17280),
], trader, signTransaction);
```

For the standard MVP flow, skip approve and go straight to the trade call.

---

## Step 2 — Open a market trade

Signature:
```
open_market_trade(trader, pair_index, is_long, collateral, leverage, tp_price, sl_price) -> u32
```
Returns the assigned `trade_index`. Collateral is pulled immediately; the
position opens at the current oracle price. An open fee (group `open_fee_p`) is
deducted from collateral before the position is sized.

```ts
async function openMarket(opts: {
  trader: string; pairIndex: number; isLong: boolean;
  collateralUsdc: number; leverage: number;
  tpHuman?: number; slHuman?: number;   // 0 / omit = no trigger
  signTransaction: any;
}) {
  const { returnValue } = await invoke(PM_ID, "open_market_trade", [
    addr(opts.trader),
    u32(opts.pairIndex),
    bool(opts.isLong),
    i128(toUsdc(opts.collateralUsdc)),
    u32(opts.leverage),
    i128(opts.tpHuman ? toPrice(opts.tpHuman) : 0n),
    i128(opts.slHuman ? toPrice(opts.slHuman) : 0n),
  ], opts.trader, opts.signTransaction);

  return returnValue as number; // trade_index
}
```

### TP/SL semantics

- `tp_price` / `sl_price` of `0` means "no trigger".
- **Long:** TP fires when price ≥ tp; SL fires when price ≤ sl.
- **Short:** TP fires when price ≤ tp; SL fires when price ≥ sl.
- Triggers are executed by an off-chain keeper calling `execute_tp_sl`. They are
  not instantaneous — they fire on the next oracle tick that crosses the level.
- Validate in the UI: for a long, `sl < openPrice < tp`; for a short,
  `tp < openPrice < sl`. The contract does not reject crossed values, so guard
  client-side.

---

## Step 3 — Place a limit order

Signature:
```
place_limit_order(trader, pair_index, is_long, collateral, leverage, limit_price, tp_price, sl_price) -> u32
```
Returns `limit_index`. Collateral is pulled immediately and held by the PM until
the order executes or is canceled. A keeper calls `execute_limit_order` once the
oracle price crosses `limit_price`.

```ts
async function placeLimit(opts: {
  trader: string; pairIndex: number; isLong: boolean;
  collateralUsdc: number; leverage: number;
  limitHuman: number; tpHuman?: number; slHuman?: number;
  signTransaction: any;
}) {
  const { returnValue } = await invoke(PM_ID, "place_limit_order", [
    addr(opts.trader),
    u32(opts.pairIndex),
    bool(opts.isLong),
    i128(toUsdc(opts.collateralUsdc)),
    u32(opts.leverage),
    i128(toPrice(opts.limitHuman)),
    i128(opts.tpHuman ? toPrice(opts.tpHuman) : 0n),
    i128(opts.slHuman ? toPrice(opts.slHuman) : 0n),
  ], opts.trader, opts.signTransaction);

  return returnValue as number; // limit_index
}
```

Execution rule (keeper-enforced):
- **Long limit:** executes when oracle price ≤ `limit_price` (buy the dip).
- **Short limit:** executes when oracle price ≥ `limit_price`.

### Update a limit order

```
update_limit_order(trader, pair_index, limit_index, limit_price, tp_price, sl_price)
```
```ts
await invoke(PM_ID, "update_limit_order", [
  addr(trader), u32(pairIndex), u32(limitIndex),
  i128(toPrice(newLimit)), i128(toPrice(newTp)), i128(toPrice(newSl)),
], trader, signTransaction);
```

### Cancel a limit order (refunds full collateral)

```
cancel_limit_order(trader, pair_index, limit_index)
```
```ts
await invoke(PM_ID, "cancel_limit_order", [
  addr(trader), u32(pairIndex), u32(limitIndex),
], trader, signTransaction);
```

---

## Step 4 — Set / update TP-SL on an open trade

Signature:
```
update_tp_sl(trader, pair_index, trade_index, tp_price, sl_price)
```
Overwrites both values. Pass `0` to clear a side.

```ts
async function setTpSl(opts: {
  trader: string; pairIndex: number; tradeIndex: number;
  tpHuman: number; slHuman: number; signTransaction: any;
}) {
  await invoke(PM_ID, "update_tp_sl", [
    addr(opts.trader),
    u32(opts.pairIndex),
    u32(opts.tradeIndex),
    i128(opts.tpHuman ? toPrice(opts.tpHuman) : 0n),
    i128(opts.slHuman ? toPrice(opts.slHuman) : 0n),
  ], opts.trader, opts.signTransaction);
}
```

---

## Step 5 — Close a market trade

Signature:
```
close_market_trade(trader, pair_index, trade_index)
```
Settles at the current oracle price. A close fee is taken; net PnL is settled
with the Vault and paid to the trader.

```ts
await invoke(PM_ID, "close_market_trade", [
  addr(trader), u32(pairIndex), u32(tradeIndex),
], trader, signTransaction);
```

> Liquidation (`liquidate_trade`) and TP/SL execution (`execute_tp_sl`) are
> keeper-only operations. The frontend does not call them — it observes the
> results via the backend WebSocket / history API.

---

## Reading state

Views are simulate-only — no signing, no fees, instant.

```ts
async function readView(contractId: string, method: string, args: any[], from: string) {
  const account = await server.getAccount(from);
  const tx = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: NETWORK })
    .addOperation(new Contract(contractId).call(method, ...args))
    .setTimeout(30)
    .build();
  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) throw new Error(sim.error);
  return scValToNative(sim.result!.retval);
}
```

### Get one trade
```ts
const trade = await readView(PM_ID, "get_trade", [
  addr(trader), u32(pairIndex), u32(tradeIndex),
], trader);
// { pair_index, is_long, leverage, open_price, collateral, tp_price, sl_price, ... }
```

### Live PnL (oracle-priced)
```ts
const pnlRaw = await readView(PM_ID, "get_trade_pnl", [
  addr(trader), u32(pairIndex), u32(tradeIndex),
], trader);
const pnlUsdc = fromUsdc(pnlRaw); // signed; negative = loss
```

### Reading pair config
```ts
const pair = await readView(REGISTRY_ID, "get_pair", [u32(pairIndex)], trader);
// { symbol, group_index, min_leverage, max_leverage, max_oi_usdc,
//   liq_threshold_p, max_gain_p, disabled, ... }
const group = await readView(REGISTRY_ID, "get_group", [u32(pair.group_index)], trader);
// { name, max_collateral_usdc, open_fee_p, close_fee_p }
const oi = await readView(REGISTRY_ID, "get_oi", [u32(pairIndex)], trader);
// { long, short }
```

Use these to:
- bound the leverage slider to `[min_leverage, max_leverage]`,
- show the open/close fee preview (`collateral * leverage * open_fee_p / 1e10`),
- disable trading when `pair.disabled === true`.

---

## Estimating fees & liquidation in the UI

```ts
// open fee (USDC, on-chain scale) = notional * open_fee_p / P_SCALE
const notional = toUsdc(collateral) * BigInt(leverage);
const openFee  = notional * BigInt(group.open_fee_p) / PRICE_SCALE; // P_SCALE == 1e10

// approximate liquidation price (no funding/rollover in MVP):
//  long:  open * (1 - liq_threshold_p/100 / leverage)
//  short: open * (1 + liq_threshold_p/100 / leverage)
function liqPrice(openHuman: number, isLong: boolean, leverage: number, liqThresholdP: number) {
  const move = (liqThresholdP / 100) / leverage;
  return isLong ? openHuman * (1 - move) : openHuman * (1 + move);
}
```

For an authoritative liq price use the registry view
`get_trade_liquidation_price(trade_meta, at_ledger)`.

---

## Error handling

Simulation/exec failures surface the contract error code. Map
`PositionManagerError` (`#[repr(u32)]`) to user messages:

| Code | Name | UI message |
|------|------|------------|
| 4  | Paused | Trading is paused |
| 5  | PairDisabled | This market is disabled |
| 6  | LeverageIncorrect | Leverage out of allowed range |
| 7  | AboveMaxPos | Position exceeds max size |
| 8  | BelowMinPos | Position below minimum |
| 9  | MaxTradesReached | Too many open trades on this pair |
| 12 | WrongTp | Invalid take-profit |
| 13 | WrongSl | Invalid stop-loss |
| 14 | OiCapExceeded | Market open-interest cap reached |
| 17 | TradeNotFound | Trade no longer exists |
| 21 | PriceMismatch | Price moved; order not triggered |
| 23 | NotLiquidatable | Position is not liquidatable |
| 26 | InvalidParam | Invalid input |
| 27 | OracleUnavailable | Price feed unavailable, try again |
| 28 | LimitNotFound | Limit order no longer exists |

```ts
function parseContractError(raw: string): string {
  const m = raw.match(/Error\(Contract, #(\d+)\)/);
  const map: Record<number, string> = {
    4: "Trading is paused", 5: "This market is disabled",
    6: "Leverage out of allowed range", 9: "Too many open trades on this pair",
    17: "Trade no longer exists", 21: "Price moved; order not triggered",
    26: "Invalid input", 27: "Price feed unavailable, try again",
    28: "Limit order no longer exists",
  };
  return m ? (map[+m[1]] ?? `Contract error #${m[1]}`) : raw;
}
```

---

## End-to-end flow

1. Connect wallet → get `trader` address + `signTransaction`.
2. Fetch pair + group config from the registry (cache it).
3. Check the trader's USDC balance covers `collateral`.
4. Build trade params; convert with `toUsdc` / `toPrice`.
5. Call `open_market_trade` / `place_limit_order` via `invoke`.
6. On success, read the returned `trade_index` / `limit_index`.
7. Subscribe to the backend WebSocket for live position + price updates
   (see `lumenliquid-backend/docs/api-ws-instruct.md`).
8. Close via `close_market_trade`, or let keeper TP/SL/liq fire.

For live data (open trades, history, prices) use the backend services at
`https://services.lumenliquid.xyz` rather than polling the chain.

---

## Live data — backend services

The chain is the source of truth for writes; reads come from the backend so the
UI stays fast and you avoid polling Soroban. Full schema in
`lumenliquid-backend/docs/api-ws-instruct.md`.

| Use | Endpoint |
|-----|----------|
| Price feed (all pairs) | `wss://services.lumenliquid.xyz/ws/v1/prices` |
| One trader's trades (realtime) | `wss://services.lumenliquid.xyz/ws/v1/trades/{trader}` |
| All traders' trades (realtime) | `wss://services.lumenliquid.xyz/ws/v1/trades` |
| Trading history (REST, paged) | `https://services.lumenliquid.xyz/api/v1/trading-history/{trader}` |

### Price feed

Plain-text messages, not JSON. Format `{pairIndex}|{price}` (price is human
decimal, e.g. `0|67123.45`). Use this to drive live PnL and entry previews.

```ts
const px = new WebSocket("wss://services.lumenliquid.xyz/ws/v1/prices");
px.onmessage = (e) => {
  const [pairIndex, price] = (e.data as string).split("|");
  setPrice(Number(pairIndex), parseFloat(price));
};
```

### Trade feeds

On connect, the server pushes a full `snapshot` of open trades; every state
change (open, close, liq, TP/SL exec, TP/SL update) pushes a fresh snapshot.

```ts
const ws = new WebSocket(`wss://services.lumenliquid.xyz/ws/v1/trades/${trader}`);
ws.onmessage = (e) => {
  const msg = JSON.parse(e.data); // { type: "snapshot", trades: [...], pairs: [...] }
  if (msg.type === "snapshot") setOpenTrades(msg.trades);
};
```

Use `/ws/v1/trades` (no trader) for a market-wide feed of every open position.
Prices in snapshots are at on-chain scale (`1e10`); convert with `fromPrice`.

### Trading history (paged)

```
GET /api/v1/trading-history/{trader}?limit=20&cursor=<RFC3339>
```

| Query | Default | Notes |
|-------|---------|-------|
| `limit`  | 20 | page size, max 100 |
| `cursor` | — | `closed_at` RFC3339 timestamp from the previous page's `next_cursor` |

Response: `{ trader, history: [...], next_cursor, has_more }`. Each row carries
`close_reason` ∈ `manual` \| `tp` \| `sl` \| `liquidation`, plus
`realized_pnl`, `open_fee`, `close_fee`, `open_price`, `close_price`,
`opened_tx`, `closed_tx`. Amounts are on-chain scale — convert with `fromUsdc` /
`fromPrice`.

```ts
async function fetchHistory(trader: string, cursor?: string) {
  const u = new URL(`https://services.lumenliquid.xyz/api/v1/trading-history/${trader}`);
  u.searchParams.set("limit", "20");
  if (cursor) u.searchParams.set("cursor", cursor);
  return (await fetch(u)).json(); // { history, next_cursor, has_more }
}

// infinite scroll: keep passing next_cursor until has_more === false
let cursor: string | undefined;
do {
  const page = await fetchHistory(trader, cursor);
  appendRows(page.history);
  cursor = page.has_more ? page.next_cursor : undefined;
} while (cursor);
```

