package com.acme.sample;

import java.util.List;

import org.springframework.stereotype.Service;

/**
 * Business logic for users.
 */
@Service
public class UserService {
    public List<User> findAll() {
        return List.of();
    }

    public User find(Long id) {
        return null;
    }

    public User find(String name) {
        return null;
    }
}
