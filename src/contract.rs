#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_binary, Addr, Binary, Deps, DepsMut, Env, Event, MessageInfo, Response, StdResult,
};

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{PendingTransactions, Transaction, ADMINS, PENDING_TXS, QUORUM, SIGNED_TX};

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    if msg.owners.len() == 0 {
        return Err(ContractError::ZeroOwners);
    }

    if msg.quorum > msg.owners.len() as u32 {
        return Err(ContractError::WrongQuorum {
            quorum: msg.quorum,
            owners: msg.owners.len() as u32,
        });
    }

    ADMINS.save(deps.storage, &msg.owners)?;
    QUORUM.save(deps.storage, &msg.quorum)?;
    let pending = PendingTransactions::new(Vec::new());
    PENDING_TXS.save(deps.storage, &pending)?;
    SIGNED_TX.save(deps.storage, (Addr::unchecked("test"), 0), &false)?;

    let events = msg
        .owners
        .into_iter()
        .map(|owner| Event::new("owner-added").add_attribute("addr", owner));

    Ok(Response::new().add_events(events))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    exec::is_admin(&deps, &info)?;
    match msg {
        ExecuteMsg::CreateTransaction { to, coins } => exec::create_tx(deps, info, to, coins),
        ExecuteMsg::SignTransactions { tx_id } => exec::sign_tx(deps, info, tx_id),
        ExecuteMsg::ExecuteTransaction { tx_id } => exec::execute_tx(deps, tx_id),
    }
}

mod exec {
    use super::*;
    use crate::state::{PENDING_TXS, SIGNED_TX};
    use cosmwasm_std::{Addr, BankMsg, Coin};

    pub fn create_tx(
        deps: DepsMut,
        info: MessageInfo,
        to: Addr,
        coins: Vec<Coin>,
    ) -> Result<Response, ContractError> {
        let admins = ADMINS.load(deps.storage)?;

        if !admins.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        let mut pending_txs = PENDING_TXS.load(deps.storage)?;
        let next_id = pending_txs.next_id();
        let mut tx = Transaction::new(to, next_id, coins);
        tx.num_confirmations = 1;
        pending_txs.push(tx.clone());
        PENDING_TXS.save(deps.storage, &pending_txs)?;

        // Since the user proposed the tx he already approves that it will be executed,
        // This way he won't have to approve the transaction again
        SIGNED_TX.save(deps.storage, (info.sender, next_id), &true)?;
        Ok(Response::new().add_event(Event::new("new_tx").add_attribute("tx", tx.to_string())))
    }

    pub fn sign_tx(
        deps: DepsMut,
        info: MessageInfo,
        tx_id: u32,
    ) -> Result<Response, ContractError> {
        if let Ok(signed) = SIGNED_TX.load(deps.storage, (info.sender.clone(), tx_id)) {
            if signed {
                return Err(ContractError::AlreadySigned(tx_id));
            }
        }

        SIGNED_TX.save(deps.storage, (info.sender, tx_id), &true)?;

        let mut pending_txs = PENDING_TXS.load(deps.storage)?;

        let tx = pending_txs
            .find_mut(tx_id)
            .ok_or(ContractError::NonExistentTx(tx_id))?;

        tx.num_confirmations += 1;

        PENDING_TXS.save(deps.storage, &pending_txs)?;

        Ok(Response::new())
    }

    pub fn execute_tx(deps: DepsMut, tx_id: u32) -> Result<Response, ContractError> {
        let pending_txs = PENDING_TXS.load(deps.storage)?;

        let tx = pending_txs
            .find(tx_id)
            .ok_or(ContractError::NonExistentTx(tx_id))?;

        let quorum = QUORUM.load(deps.storage)?;

        if quorum > tx.num_confirmations {
            return Err(ContractError::NotEnoughSignatures {
                quorum,
                num_signed: tx.num_confirmations,
            });
        }

        let message = BankMsg::Send {
            to_address: tx.to.to_string(),
            amount: tx.coins.clone(),
        };

        Ok(Response::new().add_message(message))
    }

    pub fn is_admin(deps: &DepsMut, info: &MessageInfo) -> Result<(), ContractError> {
        let admins = ADMINS.load(deps.storage)?;
        if !admins.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        Ok(())
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::ListAdmins {} => to_binary(&query::list_admins(deps)?),
        QueryMsg::ListPending {} => to_binary(&query::list_pending(deps)?),
        QueryMsg::ListSigned { admin, tx_id } => {
            to_binary(&query::list_signed(deps, admin, tx_id)?)
        }
    }
}

mod query {
    use super::*;
    use crate::{
        msg::{ListAdminsResp, ListPendingResp, ListSignedResp},
        state::{PENDING_TXS, SIGNED_TX},
    };
    use cosmwasm_std::Addr;

    pub fn list_signed(deps: Deps, admin: Addr, tx_id: u32) -> StdResult<ListSignedResp> {
        let signed = SIGNED_TX.load(deps.storage, (admin, tx_id))?;

        Ok(ListSignedResp { signed })
    }

    pub fn list_admins(deps: Deps) -> StdResult<ListAdminsResp> {
        let admins = ADMINS.load(deps.storage)?;
        Ok(ListAdminsResp { admins })
    }

    pub fn list_pending(deps: Deps) -> StdResult<ListPendingResp> {
        let transactions = PENDING_TXS.load(deps.storage)?;

        Ok(ListPendingResp { transactions })
    }
}

#[cfg(test)]
mod tests {
    use crate::msg::{ListPendingResp, ListSignedResp};

    use super::*;
    use cosmwasm_std::{coins, Addr, Coin};
    use cw_multi_test::{App, ContractWrapper, Executor};

    fn instantiate_contract() -> (Addr, App) {
        let mut app = App::new(|router, _, storage| {
            router
                .bank
                .init_balance(storage, &Addr::unchecked("owner"), coins(5, "atom"))
                .unwrap();
        });

        let code = ContractWrapper::new(execute, instantiate, query);
        let code_id = app.store_code(Box::new(code));

        let coin = Coin::new(5, "atom");

        let addr = app
            .instantiate_contract(
                code_id,
                Addr::unchecked("owner"),
                &InstantiateMsg {
                    owners: vec![
                        Addr::unchecked("owner1"),
                        Addr::unchecked("owner2"),
                        Addr::unchecked("owner3"),
                    ],
                    quorum: 2,
                },
                &[coin],
                "Multisig",
                None,
            )
            .unwrap();

        (addr, app)
    }

    #[test]
    fn test_instantiate() {
        let (addr, app) = instantiate_contract();

        let balance: Vec<Coin> = app.wrap().query_all_balances(&addr).unwrap();
        assert_eq!(vec![Coin::new(5, "atom")], balance);
    }

    #[test]
    #[should_panic(expected = "Unauthorized")]
    fn test_propose_unauthorized() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("unathorized"), addr.clone(), &msg, &[])
            .unwrap();
    }

    #[test]
    fn test_propose() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();
        let msg = QueryMsg::ListPending {};

        let resp: ListPendingResp = app.wrap().query_wasm_smart(addr, &msg).unwrap();
        let mut tx = Transaction::new(Addr::unchecked("owner"), 0, vec![Coin::new(5, "atom")]);
        tx.num_confirmations = 1;
        assert_eq!(&tx, resp.transactions.index(0).unwrap());
    }

    #[test]
    #[should_panic(expected = "You already signed transaction with id: 0")]
    fn test_sign_after_already_signed() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();

        let msg = ExecuteMsg::SignTransactions { tx_id: 0 };

        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();
    }

    #[test]
    fn test_sign() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();

        let msg = ExecuteMsg::SignTransactions { tx_id: 0 };

        app.execute_contract(Addr::unchecked("owner2"), addr.clone(), &msg, &[])
            .unwrap();
        app.execute_contract(Addr::unchecked("owner3"), addr.clone(), &msg, &[])
            .unwrap();

        let resp_owner1: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner2"),
                    tx_id: 0,
                },
            )
            .unwrap();

        let resp_owner2: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner2"),
                    tx_id: 0,
                },
            )
            .unwrap();

        let resp_owner3: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner3"),
                    tx_id: 0,
                },
            )
            .unwrap();

        assert_eq!(resp_owner1.signed, true);
        assert_eq!(resp_owner2.signed, true);
        assert_eq!(resp_owner3.signed, true);

        let resp: ListPendingResp = app
            .wrap()
            .query_wasm_smart(addr, &QueryMsg::ListPending {})
            .unwrap();

        assert_eq!(resp.transactions.index(0).unwrap().num_confirmations, 3);
    }

    #[test]
    #[should_panic(
        expected = "Not enough admins signed this transaction, the quorum is 2 and only 1 signed the transaction"
    )]
    fn test_execute_under_quorum() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();

        let msg = ExecuteMsg::ExecuteTransaction { tx_id: 0 };

        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();
    }

    #[test]
    fn test_execute() {
        let (addr, mut app) = instantiate_contract();

        let msg = ExecuteMsg::CreateTransaction {
            to: Addr::unchecked("owner"),
            coins: vec![Coin::new(5, "atom")],
        };
        app.execute_contract(Addr::unchecked("owner1"), addr.clone(), &msg, &[])
            .unwrap();

        let msg = ExecuteMsg::SignTransactions { tx_id: 0 };

        app.execute_contract(Addr::unchecked("owner2"), addr.clone(), &msg, &[])
            .unwrap();
        app.execute_contract(Addr::unchecked("owner3"), addr.clone(), &msg, &[])
            .unwrap();

        let resp_owner1: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner2"),
                    tx_id: 0,
                },
            )
            .unwrap();

        let resp_owner2: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner2"),
                    tx_id: 0,
                },
            )
            .unwrap();

        let resp_owner3: ListSignedResp = app
            .wrap()
            .query_wasm_smart(
                addr.clone(),
                &QueryMsg::ListSigned {
                    admin: Addr::unchecked("owner3"),
                    tx_id: 0,
                },
            )
            .unwrap();

        assert_eq!(resp_owner1.signed, true);
        assert_eq!(resp_owner2.signed, true);
        assert_eq!(resp_owner3.signed, true);

        let msg = ExecuteMsg::ExecuteTransaction { tx_id: 0 };

        app.execute_contract(Addr::unchecked("owner3"), addr.clone(), &msg, &[])
            .unwrap();

        let balance: Coin = app.wrap().query_balance(&addr, "atom").unwrap();
        assert_eq!(Coin::new(0, "atom"), balance);

        let balance: Coin = app
            .wrap()
            .query_balance(&Addr::unchecked("owner"), "atom")
            .unwrap();
        assert_eq!(Coin::new(5, "atom"), balance);
    }
}
