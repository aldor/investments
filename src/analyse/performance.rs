use core::GenericResult;
use currency::CashAssets;
use currency::converter::CurrencyConverter;
#[cfg(test)] use currency::converter::CurrencyConverterBackend;
use types::{Date, Decimal};
use util;

use super::deposit_emulator::DepositEmulator;

// FIXME: Support:
// * Withdrawals
// * Take taxes into account
// * Deposit fees
/// Calculates average rate of return from cash investments by comparing portfolio performance to
/// performance of bank deposit with the exactly same investments and monthly capitalization.
pub fn get_average_rate_of_return(
    deposits: &Vec<CashAssets>, current_assets: CashAssets, currency: &str,
    converter: &CurrencyConverter
) -> GenericResult<Decimal> {
    let mut transactions = Vec::<CashAssets>::new();

    for deposit in deposits {
        if deposit.date > current_assets.date {
            return Err!("Got a deposit from the future ({})", util::format_date(deposit.date));
        }

        assert!(deposit.cash.is_positive());
        transactions.push(*deposit);
    }

    transactions.sort_by_key(|assets| assets.date);

    // FIXME: Support custom starting point
    assert_ne!(transactions.len(), 0);
    let start_date = transactions[0].date;
    let start_assets = dec!(0);

    let result_assets = converter.convert_to(current_assets.date, current_assets.cash, currency)?;

    // FIXME: DepositEmulator shouldn't know anything about currency conversion
    let emulate = |interest: Decimal| -> GenericResult<Decimal> {
        let assets = DepositEmulator::emulate(
            start_date, start_assets, &transactions, current_assets.date, currency, interest,
            converter)?;

        let difference = (result_assets - assets).abs();

        Ok(difference)
    };

    let mut interest = dec!(0);
    let mut difference = emulate(interest)?;

    for mut step in [decs!("1"), decs!("0.1"), decs!("0.01")].iter().cloned() {
        let decreasing_difference = emulate(interest - step)?;
        let increasing_difference = emulate(interest + step)?;

        if decreasing_difference > difference && difference < increasing_difference {
            return Ok(interest);
        }

        if decreasing_difference < increasing_difference {
            assert!(decreasing_difference < difference);
            step = -step;
        } else if decreasing_difference > increasing_difference {
            assert!(increasing_difference < difference);
        } else {
            unreachable!();
        }

        interest += step;

        loop {
            let next_interest = interest + step;
            let next_difference = emulate(next_interest)?;

            if next_difference > difference {
                break;
            }

            difference = next_difference;
            interest = next_interest;
        }
    }

    Ok(interest)
}

// FIXME: Deprecate
// FIXME: Support:
// * Withdrawals
// * Non-zero starting point
// * Compare with complex interest
// * Calculate taxes
pub fn get_average_profit(
    deposits: &Vec<CashAssets>, current_assets: CashAssets, currency: &str,
    converter: &CurrencyConverter
) -> GenericResult<Decimal> {
    // Calculates average profit from cash income. Splits the whole period into intervals, where
    // we have a "constant" assets in each interval.
    //
    // profit = current_assets - total_income
    // (assets * days + assets * days + ...) * interest = profit

    let mut total_income = dec!(0);
    let mut relative_contributions = dec!(0);

    let mut transactions = Vec::<CashAssets>::new();

    for deposit in deposits {
        if deposit.date > current_assets.date {
            continue;
        }

        assert!(deposit.cash.is_positive());
        transactions.push(*deposit);
    }

    transactions.sort_by_key(|assets| assets.date);

    for (index, assets) in transactions.iter().enumerate() {
        total_income += converter.convert_to(assets.date, assets.cash, currency)?;
        if total_income < dec!(0) {
            return Err!("Portfolio got negative balance on {}", util::format_date(assets.date));
        }

        let end_date = if index < deposits.len() - 1 {
            transactions[index + 1].date
        } else {
            current_assets.date
        };

        let days = (end_date - assets.date).num_days();
        relative_contributions += total_income * Decimal::from(days);
    }

    if relative_contributions == dec!(0) {
        return Err!("There are no deposits for the specified period")
    }

    let converted_current_assets = converter.convert_to(
        current_assets.date, current_assets.cash, currency)?;

    let profit = converted_current_assets - total_income;
    let interest = profit / relative_contributions * dec!(365);

    Ok(interest)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! basic_tests {
        ($($name:ident: $args:expr,)*) => {
        $(
            #[test]
            fn $name() {
                let (other_currency, other_amount) = $args;
                basic(other_currency, other_amount);
            }
        )*
        }
    }

    basic_tests! {
        basic_rub: ("RUB", dec!(100)),
        basic_usd: ("USD", dec!(1)),
    }

    fn basic(other_currency: &str, other_amount: Decimal) {
        struct ConverterMock {}
        impl CurrencyConverterBackend for ConverterMock {
            fn convert(&self, from: &str, to: &str, _date: Date, amount: Decimal) -> GenericResult<Decimal> {
                Ok(match (from, to) {
                    ("RUB", "RUB") => amount,
                    ("USD", "RUB") => amount * dec!(100),
                    _ => unreachable!(),
                })
            }
        }

        let currency = "RUB";
        let converter = CurrencyConverter::new_with_backend(Box::new(ConverterMock {}));

        let deposits = vec![
            CashAssets::new(date!(10, 1, 2013), currency, dec!(100)),
            CashAssets::new(date!(10, 5, 2014), other_currency, other_amount),
        ];

        let year_interest = decs!("0.12");
        let month_interest = year_interest / dec!(12);

        // Emulate a bank deposit with 12% interest and capitalization on each income

        let current_assets =
            dec!(200) +
            dec!(100) * month_interest * dec!(16) +
            (dec!(200) + dec!(100) * month_interest * dec!(16)) * month_interest * dec!(8);
        assert_eq!(current_assets, decs!("233.28"));

        let year_interest_with_capitalization =
            (current_assets - dec!(200)) / Decimal::from(16 * 100 + 8 * 200) * dec!(12);
        assert_eq!(year_interest_with_capitalization, decs!("0.1248"));

        let current_assets = CashAssets::new(date!(10, 1, 2015), currency, current_assets);
        let average_interest = get_average_profit(
            &deposits, current_assets, currency, &converter).unwrap();

        assert!(year_interest < average_interest);
        assert!(average_interest < year_interest_with_capitalization);
    }

    macro_rules! currency_rate_change_tests {
        ($($name:ident: $arg:expr,)*) => {
        $(
            #[test]
            fn $name() {
                currency_rate_change($arg);
            }
        )*
        }
    }

    currency_rate_change_tests! {
        currency_rate_change_rub: "RUB",
        currency_rate_change_usd: "USD",
    }

    fn currency_rate_change(currency: &str) {
        struct ConverterMock {}
        impl CurrencyConverterBackend for ConverterMock {
            fn convert(&self, from: &str, to: &str, date: Date, amount: Decimal) -> GenericResult<Decimal> {
                let price = Decimal::from(match date {
                    date if date == date!(1, 4, 2018) => 100,
                    date if date == date!(1, 5, 2018) => 200,
                    date if date == date!(1, 6, 2018) => 400,
                    date if date == date!(1, 7, 2018) => 800,
                    _ => unreachable!(),
                });

                if from == to {
                    return Ok(amount);
                }

                Ok(match (from, to) {
                    ("USD", "RUB") => amount * price,
                    ("RUB", "USD") => amount / price,
                    _ => unreachable!(),
                })
            }
        }

        let converter = CurrencyConverter::new_with_backend(Box::new(ConverterMock {}));

        let deposits = vec![
            CashAssets::new(date!(1, 4, 2018), "RUB", dec!(100)),
            CashAssets::new(date!(1, 5, 2018), "RUB", dec!(200)),
            CashAssets::new(date!(1, 6, 2018), "USD", dec!(2)),
        ];
        let current_assets = CashAssets::new(date!(1, 7, 2018), "USD", dec!(4));

        let average_interest = get_average_profit(
            &deposits, current_assets, currency, &converter).unwrap();

        if currency == "RUB" {
            assert!(average_interest > decs!("16.8") && average_interest < dec!(17));
        } else if currency == "USD" {
            assert_eq!(average_interest, dec!(0));
        } else {
            unreachable!();
        }
    }
}
