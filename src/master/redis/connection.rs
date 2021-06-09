/*
 * mCaptcha - A proof of work based DoS protection system
 * Copyright © 2021 Aravinth Manivannan <realravinth@batsense.net>
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of the
 * License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Affero General Public License for more details.
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */
use redis::Value;

use crate::errors::*;
use crate::master::messages::{AddSite, AddVisitor};
use crate::master::AddVisitorResult;
use crate::master::CreateMCaptcha;
use crate::redis::Redis;
use crate::redis::RedisConfig;
use crate::redis::RedisConnection;

/// Redis instance with mCaptcha Redis module loaded
pub struct MCaptchaRedis(Redis);

/// Connection to Redis instance with mCaptcha Redis module loaded
pub struct MCaptchaRedisConnection(RedisConnection);

const GET: &str = "MCAPTCHA_CACHE.GET";
const ADD_VISITOR: &str = "MCAPTCHA_CACHE.ADD_VISITOR";
const DEL: &str = "MCAPTCHA_CACHE.DELETE_CAPTCHA";
const ADD_CAPTCHA: &str = "MCAPTCHA_CACHE.ADD_CAPTCHA";
const CAPTCHA_EXISTS: &str = "MCAPTCHA_CACHE.CAPTCHA_EXISTS";

const MODULE_NAME: &str = "mcaptcha_cahce";

impl MCaptchaRedis {
    /// Get new [MCaptchaRedis]. Use this when executing commands that are
    /// only supported by mCaptcha Redis module. Internally, when object
    /// is created, checks are performed to check if the module is loaded and if
    /// the required commands are available
    pub async fn new(redis: RedisConfig) -> CaptchaResult<Self> {
        let redis = Redis::new(redis).await?;
        let m = MCaptchaRedis(redis);
        m.get_client().is_module_loaded().await?;
        Ok(m)
    }

    /// Get connection to a Redis instance with mCaptcha Redis module loaded
    ///
    /// Uses interior mutability so look out for panics!
    pub fn get_client(&self) -> MCaptchaRedisConnection {
        MCaptchaRedisConnection(self.0.get_client())
    }
}

impl MCaptchaRedisConnection {
    async fn is_module_loaded(&self) -> CaptchaResult<()> {
        let modules: Vec<Vec<String>> = self
            .0
            .exec(redis::cmd("MODULE").arg(&["LIST"]))
            .await
            .unwrap();

        for list in modules.iter() {
            match list.iter().find(|module| module.as_str() == MODULE_NAME) {
                Some(_) => (),
                None => return Err(CaptchaError::MCaptchaRedisModuleIsNotLoaded),
            }
        }

        let commands = vec![ADD_VISITOR, ADD_CAPTCHA, DEL, CAPTCHA_EXISTS, GET];

        for cmd in commands.iter() {
            match self
                .0
                .exec(redis::cmd("COMMAND").arg(&["INFO", cmd]))
                .await
                .unwrap()
            {
                Value::Bulk(mut val) => {
                    match val.pop() {
                        Some(Value::Nil) => {
                            return Err(CaptchaError::MCaptchaRediSModuleCommandNotFound(
                                cmd.to_string(),
                            ))
                        }
                        _ => (),
                    };
                }

                _ => (),
            };
        }

        Ok(())
    }

    /// Add visitor
    pub async fn add_visitor(&self, msg: AddVisitor) -> CaptchaResult<Option<AddVisitorResult>> {
        let res: String = self.0.exec(redis::cmd(ADD_VISITOR).arg(&[msg.0])).await?;
        let res: AddVisitorResult = serde_json::from_str(&res).unwrap();
        Ok(Some(res))
    }

    /// Register new mCaptcha with Redis
    pub async fn add_mcaptcha(&self, msg: AddSite) -> CaptchaResult<()> {
        let name = msg.id;
        let captcha: CreateMCaptcha = msg.mcaptcha.into();
        let payload = serde_json::to_string(&captcha).unwrap();
        self.0
            .exec(redis::cmd(ADD_CAPTCHA).arg(&[name, payload]))
            .await?;
        Ok(())
    }

    /// Check if an mCaptcha object is available in Redis
    pub async fn check_captcha_exists(&self, captcha: &str) -> CaptchaResult<bool> {
        let exists: usize = self
            .0
            .exec(redis::cmd(CAPTCHA_EXISTS).arg(&[captcha]))
            .await?;
        if exists == 1 {
            Ok(false)
        } else if exists == 0 {
            Ok(true)
        } else {
            log::error!(
                "mCaptcha redis module responded with {} when for {}",
                exists,
                CAPTCHA_EXISTS
            );
            Err(CaptchaError::MCaptchaRedisModuleError)
        }
    }

    /// Delete an mCaptcha object from Redis
    pub async fn delete_captcha(&self, captcha: &str) -> CaptchaResult<()> {
        self.0.exec(redis::cmd(DEL).arg(&[captcha])).await?;
        Ok(())
    }

    /// Get number of visitors of an mCaptcha object from Redis
    pub async fn get_visitors(&self, captcha: &str) -> CaptchaResult<usize> {
        let visitors: usize = self.0.exec(redis::cmd(GET).arg(&[captcha])).await?;
        Ok(visitors)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::master::embedded::counter::tests::get_mcaptcha;
    use crate::redis::*;

    const CAPTCHA_NAME: &str = "REDIS_CAPTCHA_TEST";
    const REDIS_URL: &str = "redis://127.0.1.1/";

    #[actix_rt::test]
    async fn redis_master_works() {
        let redis = Redis::new(RedisConfig::Single(REDIS_URL.into()))
            .await
            .unwrap();

        let r = MCaptchaRedis(redis);
        let r = r.get_client();
        {
            let _ = r.delete_captcha(CAPTCHA_NAME).await;
        }
        assert!(r.is_module_loaded().await.is_ok());
        assert!(!r.check_captcha_exists(CAPTCHA_NAME).await.unwrap());
        let add_mcaptcha_msg = AddSite {
            id: CAPTCHA_NAME.into(),
            mcaptcha: get_mcaptcha(),
        };

        assert!(r.add_mcaptcha(add_mcaptcha_msg).await.is_ok());
        assert!(r.check_captcha_exists(CAPTCHA_NAME).await.unwrap());

        let add_visitor_msg = AddVisitor(CAPTCHA_NAME.into());
        assert!(r.add_visitor(add_visitor_msg).await.is_ok());
        let visitors = r.get_visitors(CAPTCHA_NAME).await.unwrap();
        assert_eq!(visitors, 1);

        let add_visitor_msg = AddVisitor(CAPTCHA_NAME.into());
        assert!(r.add_visitor(add_visitor_msg).await.is_ok());
        let visitors = r.get_visitors(CAPTCHA_NAME).await.unwrap();
        assert_eq!(visitors, 2);

        assert!(r.delete_captcha(CAPTCHA_NAME).await.is_ok());
    }
}
