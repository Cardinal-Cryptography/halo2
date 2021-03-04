use ff::Field;
use std::iter;

use super::super::circuit::Any;
use super::{Argument, VerifyingKey};
use crate::{
    arithmetic::{CurveAffine, FieldExt},
    plonk::{self, ChallengeBeta, ChallengeGamma, ChallengeX, Error},
    poly::{multiopen::VerifierQuery, Rotation},
    transcript::TranscriptRead,
};

pub struct Committed<C: CurveAffine> {
    permutation_product_commitment: C,
}

pub struct Evaluated<C: CurveAffine> {
    permutation_product_commitment: C,
    permutation_product_eval: C::Scalar,
    permutation_product_next_eval: C::Scalar,
    permutation_evals: Vec<C::Scalar>,
}

impl Argument {
    pub(crate) fn read_product_commitment<C: CurveAffine, T: TranscriptRead<C>>(
        &self,
        transcript: &mut T,
    ) -> Result<Committed<C>, Error> {
        let permutation_product_commitment = transcript
            .read_point()
            .map_err(|_| Error::TranscriptError)?;

        Ok(Committed {
            permutation_product_commitment,
        })
    }
}

impl<C: CurveAffine> Committed<C> {
    pub(crate) fn evaluate<T: TranscriptRead<C>>(
        self,
        vkey: &VerifyingKey<C>,
        transcript: &mut T,
    ) -> Result<Evaluated<C>, Error> {
        let permutation_product_eval = transcript
            .read_scalar()
            .map_err(|_| Error::TranscriptError)?;
        let permutation_product_next_eval = transcript
            .read_scalar()
            .map_err(|_| Error::TranscriptError)?;
        let mut permutation_evals = Vec::with_capacity(vkey.commitments.len());
        for _ in 0..vkey.commitments.len() {
            permutation_evals.push(
                transcript
                    .read_scalar()
                    .map_err(|_| Error::TranscriptError)?,
            );
        }

        Ok(Evaluated {
            permutation_product_commitment: self.permutation_product_commitment,
            permutation_product_eval,
            permutation_product_next_eval,
            permutation_evals,
        })
    }
}

impl<C: CurveAffine> Evaluated<C> {
    pub(in crate::plonk) fn expressions<'a>(
        &'a self,
        vk: &'a plonk::VerifyingKey<C>,
        p: &'a Argument,
        advice_evals: &'a [C::Scalar],
        fixed_evals: &[C::Scalar],
        instance_evals: &'a [C::Scalar],
        l_0: C::Scalar,
        l_last: C::Scalar,
        l_cover: C::Scalar,
        beta: ChallengeBeta<C>,
        gamma: ChallengeGamma<C>,
        x: ChallengeX<C>,
    ) -> impl Iterator<Item = C::Scalar> + 'a {
        iter::empty()
            // l_0(X) * (1 - z(X)) = 0
            .chain(Some(
                l_0 * &(C::Scalar::one() - &self.permutation_product_eval),
            ))
            // l_last(X) * (1 - z(X)) = 0
            .chain(Some(
                l_last * &(C::Scalar::one() - &self.permutation_product_eval),
            ))
            // (1 - (l_cover(X) + l_last(X))) * (
            // z(omega X) \prod (p(X) + \beta s_i(X) + \gamma)
            // - z(X) \prod (p(X) + \delta^i \beta X + \gamma)
            // )
            .chain(Some({
                let mut left = self.permutation_product_next_eval;
                for (eval, permutation_eval) in p
                    .columns
                    .iter()
                    .map(|&column| match column.column_type() {
                        Any::Advice => {
                            advice_evals[vk.cs.get_any_query_index(column, Rotation::cur())]
                        }
                        Any::Fixed => {
                            fixed_evals[vk.cs.get_any_query_index(column, Rotation::cur())]
                        }
                        Any::Instance => {
                            instance_evals[vk.cs.get_any_query_index(column, Rotation::cur())]
                        }
                    })
                    .zip(self.permutation_evals.iter())
                {
                    left *= &(eval + &(*beta * permutation_eval) + &*gamma);
                }

                let mut right = self.permutation_product_eval;
                let mut current_delta = *beta * &*x;
                for eval in p.columns.iter().map(|&column| match column.column_type() {
                    Any::Advice => advice_evals[vk.cs.get_any_query_index(column, Rotation::cur())],
                    Any::Fixed => fixed_evals[vk.cs.get_any_query_index(column, Rotation::cur())],
                    Any::Instance => {
                        instance_evals[vk.cs.get_any_query_index(column, Rotation::cur())]
                    }
                }) {
                    right *= &(eval + &current_delta + &*gamma);
                    current_delta *= &C::Scalar::DELTA;
                }

                (left - &right) * (C::Scalar::one() - (l_cover + l_last))
            }))
    }

    pub(in crate::plonk) fn queries<'r, 'params: 'r>(
        &'r self,
        vk: &'r plonk::VerifyingKey<C>,
        vkey: &'r VerifyingKey<C>,
        x: ChallengeX<C>,
    ) -> impl Iterator<Item = VerifierQuery<'r, 'params, C>> + Clone {
        let x_next = vk.domain.rotate_omega(*x, Rotation::next());

        iter::empty()
            // Open permutation product commitments at x and \omega x
            .chain(Some(VerifierQuery::new_commitment(
                &self.permutation_product_commitment,
                *x,
                self.permutation_product_eval,
            )))
            .chain(Some(VerifierQuery::new_commitment(
                &self.permutation_product_commitment,
                x_next,
                self.permutation_product_next_eval,
            )))
            // Open permutation commitments for each permutation argument at x
            .chain(
                vkey.commitments
                    .iter()
                    .zip(self.permutation_evals.iter())
                    .map(move |(commitment, &eval)| {
                        VerifierQuery::new_commitment(commitment, *x, eval)
                    }),
            )
    }
}
