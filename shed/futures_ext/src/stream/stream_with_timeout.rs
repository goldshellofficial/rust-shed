/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

use futures::{
    future::FutureExt,
    stream::Stream,
    task::{Context, Poll},
};
use pin_project::pin_project;
use std::pin::Pin;
use std::time::Duration;
use thiserror::Error;
use tokio::time::Delay;

/// Error returned when a StreamWithTimeout exceeds its deadline.
#[derive(Debug, Error)]
#[error("Stream timeout with duration {:?} was exceeded", .0)]
pub struct StreamTimeoutError(Duration);

/// A stream that must finish within a given duration, or it will error during poll (i.e. it must
/// yield None). The clock starts counting the first time the stream is polled.
#[pin_project]
pub struct StreamWithTimeout<S> {
    #[pin]
    inner: S,
    duration: Duration,
    done: bool,
    deadline: Option<Delay>,
}

impl<S> StreamWithTimeout<S> {
    /// Create a new [StreamWithTimeout].
    pub fn new(inner: S, duration: Duration) -> Self {
        Self {
            inner,
            duration,
            done: false,
            deadline: None,
        }
    }
}

impl<S: Stream> Stream for StreamWithTimeout<S> {
    type Item = Result<<S as Stream>::Item, StreamTimeoutError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if *this.done {
            return Poll::Ready(None);
        }

        let duration = *this.duration;

        let deadline = this
            .deadline
            .get_or_insert_with(|| tokio::time::delay_for(duration));

        match deadline.poll_unpin(cx) {
            Poll::Ready(()) => {
                *this.done = true;
                return Poll::Ready(Some(Err(StreamTimeoutError(duration))));
            }
            Poll::Pending => {
                // Continue
            }
        }

        // Keep track of whether the stream has finished, so that we don't attempt to poll the
        // deadline later if the stream has indeed finished already.
        let res = futures::ready!(this.inner.poll_next(cx));
        if res.is_none() {
            *this.done = true;
        }

        Poll::Ready(Ok(res).transpose())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use anyhow::Error;
    use futures::stream::{StreamExt, TryStreamExt};

    #[tokio::test]
    async fn test_stream_timeout() -> Result<(), Error> {
        tokio::time::pause();

        let s = async_stream::stream! {
            yield Result::<(), Error>::Ok(());
            tokio::time::advance(Duration::from_secs(2)).await;
            yield Result::<(), Error>::Ok(());
        };

        let mut s = StreamWithTimeout::new(s.boxed(), Duration::from_secs(1));

        assert!(s.try_next().await?.is_some());
        assert!(s.try_next().await.is_err());
        assert!(s.try_next().await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_stream_done_before_timeout() -> Result<(), Error> {
        tokio::time::pause();

        let s = async_stream::stream! {
            yield Result::<(), Error>::Ok(());
            yield Result::<(), Error>::Ok(());
        };

        let mut s = StreamWithTimeout::new(s.boxed(), Duration::from_secs(1));

        assert!(s.try_next().await?.is_some());
        assert!(s.try_next().await?.is_some());
        assert!(s.try_next().await?.is_none());

        tokio::time::advance(Duration::from_secs(2)).await;

        assert!(s.try_next().await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_clock_starts_at_poll() -> Result<(), Error> {
        tokio::time::pause();

        let s = async_stream::stream! {
            yield Result::<(), Error>::Ok(());
            yield Result::<(), Error>::Ok(());
        };
        let mut s = StreamWithTimeout::new(s.boxed(), Duration::from_secs(1));

        tokio::time::advance(Duration::from_secs(2)).await;

        assert!(s.try_next().await?.is_some());
        assert!(s.try_next().await?.is_some());
        assert!(s.try_next().await?.is_none());

        Ok(())
    }
}