# Off-ramp stablecoins to a bank

A step-by-step guide for operators turning a customer's stablecoins into a
bank payment. It covers opening a wallet, receiving funds, checking the
balance, and paying a beneficiary.

## How the off-ramp works

An off-ramp takes **four steps**: open a managed wallet, send stablecoins to
it, check the balance, then pay out. The partner holds the wallet and
converts the stablecoins to *fiat* when the payout leaves.[^partner]

![Flow of funds](diagram.png "Funds move from the wallet to the bank.")

Before you start, check that:

- [x] The customer is **Verified**.
- [ ] You know which stablecoin the customer will send.
- [ ] You have the beneficiary's bank details.

> **If you can't add the wallet**
> No *Add wallet* card means the customer already has every network and
> token on offer.

## Send stablecoins to the wallet

1. On the **Wallets** tab, click the card of the network.
2. Copy the address with `Copy wallet address`.
3. From the sending wallet, send the token on that network.

| Asset | Network | Minimum | Arrival |
|:------|:-------:|--------:|---------|
| USDT  | Tron    | 10,000  | 3-5 days |
| USDC  | Ethereum | 10,000 | 3-5 days |

### Check the balance by API

```rust
fn balance(wallet: &Wallet) -> Result<u64, Error> {
    // Sum confirmed deposits only.
    let total = wallet.deposits().filter(|d| d.confirmed).map(|d| d.amount).sum();
    Ok(total)
}
```

> [!WARNING]
> For a large amount, send a small test transfer first.

---

See [the first section](#how-the-off-ramp-works) or the
[Magma docs](https://example.com/docs). Edge cases: 50% * 2 = 100, a_b_c,
#hashtag, <angle>, @mention, ~tilde~~ and a line\
break.[^partner]

[^partner]: The partner is the regulated entity that holds the funds.
