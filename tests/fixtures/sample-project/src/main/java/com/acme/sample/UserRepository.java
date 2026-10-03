package com.acme.sample;

import org.springframework.data.jpa.repository.JpaRepository;

/**
 * Persistence for users.
 */
public interface UserRepository extends JpaRepository<User, Long> {
    User findByEmail(String email);
}
